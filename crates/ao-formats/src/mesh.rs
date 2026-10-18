//! Static mesh records (rdb types 1010001 and 1010026), see `docs/formats.md` § meshes.
//!
//! A record is a Funcom `ObjectArchive` ([`crate::archive`]) holding a tree of `RRefFrame_t`-derived
//! nodes. Nodes carrying a `data` reference own an `FAFTriMeshData_t` whose `SimpleMesh`es hold the
//! vertex/index buffers. Everything is baked into one [`Mesh`] in AO's left-handed D3D space
//! converted to ao-scene's right-handed space by negating Z (and reversing triangle winding).

use crate::archive::{Archive, Object};
use crate::texture::load_texture;
use anyhow::{bail, ensure, Context, Result};
use ao_rdb::RecordStore;
use ao_scene::{Blend, Instance, Mesh, Scene, Submesh, TextureKey, Vertex, IDENTITY};
use std::collections::HashMap;

mod rig;
pub use rig::NodeRig;
#[path = "mesh_effects.rs"]
mod effects;
pub use effects::{mesh_effect_attrs, parse_effect_name, MeshEffectAttr};

/// Primary static-mesh record type (full detail).
pub const MESH_TYPE: u32 = 1010001;
/// Second mesh record type sharing the ids of a subset of [`MESH_TYPE`] with fewer triangles.
pub const MESH_LOW_TYPE: u32 = 1010026;

// D3D7 render-state ids (the engine is D3D7: `_D3DMATERIAL7`, `_D3DRENDERSTATETYPE`).
const D3DRS_SRCBLEND: i32 = 19;
const D3DRS_DESTBLEND: i32 = 20;
const D3DRS_CULLMODE: i32 = 22;
const D3DRS_ZWRITEENABLE: i32 = 14;
const D3DRS_ALPHAREF: i32 = 24;
const D3DRS_ALPHAFUNC: i32 = 25;
const D3DRS_ZFUNC: i32 = 23;
const D3DRS_FOGENABLE: i32 = 28;
const D3DRS_LIGHTING: i32 = 137;
const D3DRS_ALPHATESTENABLE: i32 = 15;
const D3DRS_ALPHABLENDENABLE: i32 = 27;
const D3DRS_SPECULARENABLE: i32 = 29;
const D3DTSS_COLOROP: i32 = 1;
const D3DTSS_COLORARG1: i32 = 2;
const D3DTOP_ADD: i32 = 7;
const D3DTA_ALPHAREPLICATE: i32 = 0x20;
const D3DBLEND_ONE: i32 = 2;
const D3DBLEND_SRCALPHA: i32 = 5;
const D3DCULL_NONE: i32 = 1;
const MAX_DEPTH: usize = 64;

/// Exact FAFTexture_t names observed in native plant archives and resolved against
/// NameTable 1000010/1 (1010004 textures). No filename substring/alpha-only inference:
/// clothing such as body_leaves, signs, bark and terrain grass are not foliage.
/// ponytail: unknown plant textures stay rigid; extend this evidence-backed list when surveyed.
fn foliage_texture_name(name: &str, key: TextureKey) -> bool {
    // 21025's op2_canopy / large leaf uses this autumn-leaf atlas, not woody branches.
    (name == "branch" && key == TextureKey { rdb_type: OVERRIDE_TEXTURES, id: 257704 })
        || matches!(name,
        "leaves" | "leaves_hires" | "jungleleaves" | "leaf_opac" | "bigplantleaf"
        | "greenleaves_opac" | "yellowstarflower_opac" | "spikyshrub_opac"
        | "jungle_plantleaf2_opac" | "jungle_plantleaf_opac" | "jungle_bigfern_opac"
        | "jungle_readleafymultiple" | "pink_jungleleaf_opac" | "jungle_featheryflower_opac"
        | "applestyletrees_leaves_opac" | "applestyletrees_leaves2_opac"
        | "applestyletrees_leaves3_opac" | "applestyletrees_leaves4_opac"
        | "applestyletrees_newleaves" | "frazettatreeleaves" | "enigmatreeleaves"
        | "enigmatreeleaves2" | "enigmatreeleaves3" | "mcclean_leaves" | "dryleaves"
        | "deserttree_smallleaves_greenhue" | "deserttree_smallestleaves"
        | "big_smooth_leaf_dark" | "big_leaf_redrange_no_opac" | "blue_leaf" | "leaf"
        | "leaf_normal_64x256" | "palmeblad" | "hanging_ivy-flower_white"
        | "shrub-lightgreen-plain1" | "shrub-round1b" | "shrub-green-plain1"
        | "shrub-round1" | "shrub-yellowish-plain" | "shrub-round-green"
        | "yellow-flowers" | "red-flowers" | "yellowflower" | "flower2_128x128"
        | "plant_bigpink2_centerflower" | "plant_bigpinkpetals2" | "plant_bigpinkpetals1"
        | "plant_bigpink_centerflower" | "plant_bigbluepetals1" | "plant_bigblue_centerflower"
        | "rootplant_hair_lightgreen" | "athens_redcircleflower1" | "athens_roofgrass1"
        | "alien_leaves" | "grass_medium_high" | "grass_small" | "extra_tall_grass"
        | "extra_tall_grass_dry" | "grass_medium_high_dry" | "grass_small_dry"
        | "megatall swamptree_leaves" | "megatall swamptree_leaves2" | "shadowfern"
        | "shadowgrass001_statelversion" | "shadowgrass-light001a_statelversion"
        | "shadowgrass-light001a_statelversion_lars" | "shadowgrassgreen001_statelversion"
        | "plant_billboards_1024" | "plant_billboards_1024_lars"
        | "plant_billboards_1024_lars2" | "plant_billboards_512_lars"
        | "plant_billboards_1024_lars2_inferno" | "plant_billboards_512_lars_inferno"
        | "shrubrubbish_opacity_01" | "shrubrubbish_opacity_green01"
        | "shrubrubbish_opacity_inferno01" | "new_leaves_cluster" | "new_leaves_cluster2"
        | "new_leaves_wine-red-cluster" | "penumbra_frozen_jungletrees_leaves"
        | "nascense_plants02" | "topflowers" | "wierdwodner_leaves_opacity"
        | "notum_leaves" | "blueflower" | "pinkflower" | "creme_leaf")
}

/// Decodes static mesh record `id` (with its textures) into a scene with one instance at the origin.
pub fn load_mesh(store: &RecordStore, id: u32) -> Result<Scene> {
    let mut scene = Scene::default();
    let mesh = decode_mesh_into(store, id, &mut scene)?.with_context(|| format!("no mesh record {id}"))?;
    scene.instances.push(Instance { mesh, transform: IDENTITY });
    Ok(scene)
}

/// Loads a native animated ABIFF as rigid geometry frames rather than baking their hierarchy.
/// Each mesh index is the corresponding entry of [`NodeRig::pose_parts`]; ancestor-only frames
/// remain in the rig so their animation affects every descendant.
pub fn load_animated_mesh(store: &RecordStore, id: u32) -> Result<Option<(Scene, NodeRig)>> {
    let Some(bytes) = store.get(MESH_TYPE, id)? else { return Ok(None) };
    let Some(mut rig) = NodeRig::parse(&bytes)? else { return Ok(None) };
    let ar = Archive::parse(&bytes)?;
    let mut scene = Scene::default();
    let mut parts = Vec::new();
    fn walk(ar: &Archive, index: usize, scene: &mut Scene, parts: &mut Vec<Option<usize>>, store: &RecordStore) -> Result<()> {
        let node = &ar.objects[index]; // NodeRig has already validated the complete frame tree.
        let part = if let Some(data) = node.ref1("data") {
            let mut textures = |key| {
                if let std::collections::hash_map::Entry::Vacant(entry) = scene.textures.entry(key) {
                    if let Some(texture) = load_texture(store, key).ok().flatten() {
                        entry.insert(texture);
                    }
                }
                scene.textures.contains_key(&key)
            };
            let mut builder = Builder {
                ar, mesh: Mesh::default(), groups: HashMap::new(), have_texture: &mut textures,
                visited: vec![], object_space: true, overrides: &[], slot: 0, last_tex: None,
                generic_materials: &[],
                object_states: &[],
            };
            let state = node.ref1("delta_state").and_then(|i| ar.objects.get(i));
            for mesh in ar.objects[data].refs("mesh") {
                builder.simple_mesh(state, &ar.objects[mesh], &IDENTITY_MAT, render_node(ar, index, &ar.objects[data]))?;
            }
            builder.mesh.submeshes.retain(|s| !s.indices.is_empty());
            let index = scene.meshes.len();
            scene.meshes.push(builder.mesh);
            Some(index)
        } else {
            None
        };
        parts.push(part);
        for child in node.refs("chld") {
            walk(ar, child, scene, parts, store)?;
        }
        Ok(())
    }
    walk(&ar, ar.root, &mut scene, &mut parts, store)?;
    rig.set_parts(&parts);
    Ok(Some((scene, rig)))
}

/// Appends mesh `id` (type [`MESH_TYPE`]) and its textures (deduped by [`TextureKey`]) to `scene`;
/// returns its index in `scene.meshes`, or `None` if the record does not exist.
pub fn decode_mesh_into(store: &RecordStore, id: u32, scene: &mut Scene) -> Result<Option<usize>> {
    decode_record_into(store, MESH_TYPE, id, scene)
}

/// Like [`decode_mesh_into`] for an explicit mesh record type ([`MESH_TYPE`] or [`MESH_LOW_TYPE`]).
pub fn decode_record_into(store: &RecordStore, rdb_type: u32, id: u32, scene: &mut Scene) -> Result<Option<usize>> {
    decode_record(store, rdb_type, id, scene, false, &[], None)
}

/// Texture used by statel attribute overrides: `NewTextureData_t` ids are 1010004 records (`acg_tiles_metal_corroded_plain.png`, ...).
const OVERRIDE_TEXTURES: u32 = 1_010_004;

/// A statel's mesh (`rdb_type` = [`MESH_TYPE`] or the reduced [`MESH_LOW_TYPE`]) with its attributes applied: `(slot, texture id)` pairs replace the texture of the `slot`-th
/// `SimpleMesh` (counted over the whole node tree in traversal order, empty ones included). Statel attributes are
/// `std::vector<NewTextureData_t>` (`{i32 slot, u32 texture}`, 8 bytes) handed to `VisualMesh_t::SetMesh` (DisplaySystem
/// @0x1006b623) -> `AsyncMesh` (@0x1007125c) -> `FUN_100714c2`; see `docs/formats.md` § playfields.
///
/// `inherited`: the diffuse texture bound by whatever the native renderer last drew before this statel (`RVisual_t::ApplyStateChanges`
/// @0x1004cbe1 / `RViewPort_t::SetMaterial`'s material delta-state apply `FUN_10040aa0`, both randy31.dll: a `SimpleMesh` whose node and
/// material carry no texture channel at all issues no `SetTexture` call at all, `RDeltaState`'s texture-stage loop `FUN_1002f275` simply
/// never runs, so the GPU keeps whatever texture the previous draw call bound). Used as the starting "currently bound diffuse" for this
/// mesh's own `SimpleMesh` traversal (see `Builder::last_tex` in `decode_archive_with`), and (by the caller, `playfield::Placer`) as the
/// per-placement texture of a statel whose own archive never binds one.
pub fn decode_statel_mesh(store: &RecordStore, rdb_type: u32, id: u32, overrides: &[(u32, u32)], inherited: Option<TextureKey>, scene: &mut Scene) -> Result<Option<usize>> {
    decode_record(store, rdb_type, id, scene, false, overrides, inherited)
}

/// DisplaySystem's case-4 visual starts as `rock01.abiff` (1010001/36067).
/// `SetTriMeshData` retains that visual's animation frame; `Init` with `MeshIndex`
/// instead installs the selected data's frame (10063188, 100616e4, 1005e716).
pub(crate) fn decode_generic_mesh(store: &RecordStore, id: u32, index: Option<usize>, camera_locked: bool, object_states: &[(i32, i32)], scene: &mut Scene) -> Result<Option<usize>> {
    let Some(bytes) = store.get(MESH_TYPE, id)? else { return Ok(None) };
    let ar = Archive::parse(&bytes)?;
    let root = store.get(MESH_TYPE, 36067)?.context("missing GenericMeshObject factory mesh 1010001/36067")?;
    let root = Archive::parse(&root)?;
    let default = root.objects.iter().find(|o| o.get("anim_rot").is_some()).context("factory mesh without animation frame")?;
    let mut textures = |key| {
        if let std::collections::hash_map::Entry::Vacant(entry) = scene.textures.entry(key) {
            if let Some(texture) = load_texture(store, key).ok().flatten() {
                entry.insert(texture);
            }
        }
        scene.textures.contains_key(&key)
    };
    let mesh = generic_mesh_archive(&ar, default, index, camera_locked, object_states, &mut textures)?;
    scene.meshes.push(mesh);
    Ok(Some(scene.meshes.len() - 1))
}

fn mesh_animation_matrix(data: &Object) -> Result<Mat> {
    let rotation = data.f32s::<4>("anim_rot").context("mesh data without valid anim_rot")?;
    let position = data.f32s::<3>("anim_pos").context("mesh data without valid anim_pos")?;
    ensure!(rotation.iter().chain(position.iter()).all(|v| v.is_finite()), "nonfinite mesh animation frame");
    Ok(frame_matrix(rotation, position, 1.0))
}

fn generic_mesh_archive(ar: &Archive, default: &Object, index: Option<usize>, camera_locked: bool, object_states: &[(i32, i32)], have_texture: &mut dyn FnMut(TextureKey) -> bool) -> Result<Mesh> {
    let mut nodes = Vec::new();
    fn collect(ar: &Archive, i: usize, nodes: &mut Vec<usize>, visited: &mut [bool], depth: usize) -> Result<()> {
        ensure!(depth < MAX_DEPTH, "frame tree too deep");
        let node = ar.objects.get(i).context("dangling frame ref")?;
        ensure!(!std::mem::replace(&mut visited[i], true), "cyclic frame tree");
        if node.ref1("data").is_some() {
            nodes.push(i);
        }
        for child in node.refs("chld") {
            collect(ar, child, nodes, visited, depth + 1)?;
        }
        Ok(())
    }
    collect(ar, ar.root, &mut nodes, &mut vec![false; ar.objects.len()], 0)?;
    // GenericMesh resource loader10061505 resets the first SimpleMesh's material
    // in every data node before MeshIndex selection. References share identity:
    // every use of that material must see the reset, not just submesh slot0.
    let generic_materials: Vec<usize> = nodes.iter().filter_map(|&i| {
        let data = ar.objects.get(ar.objects[i].ref1("data")?)?;
        ar.objects.get(*data.refs("mesh").first()?)?.ref1("material")
    }).collect();
    let selected = match index {
        Some(i) => std::slice::from_ref(nodes.get(i).context("MeshIndex out of range")?),
        None => nodes.as_slice(),
    };
    let mut builder = Builder {
        ar, mesh: Mesh::default(), groups: HashMap::new(), have_texture,
        visited: vec![], object_space: true, overrides: &[], slot: 0, last_tex: None,
        generic_materials: &generic_materials,
        object_states,
    };
    for &i in selected {
        let node = &ar.objects[i];
        let data = ar.objects.get(node.ref1("data").context("mesh without data")?).context("dangling data ref")?;
        // Multiple meshes become Child_Mesh_Misc objects with explicit MeshIndex.
        let frame = if index.is_some() || nodes.len() > 1 { data } else { default };
        let mut matrix = mesh_animation_matrix(frame)?;
        // Process1005c334 replaces translation with camera + Position.
        if camera_locked {
            matrix[3] = [0.0, 0.0, 0.0, 1.0];
        }
        let state = node.ref1("delta_state").and_then(|i| ar.objects.get(i));
        for mesh in data.refs("mesh") {
            builder.simple_mesh(state, ar.objects.get(mesh).context("dangling mesh ref")?, &matrix, (None, None))?;
        }
    }
    builder.mesh.submeshes.retain(|s| !s.indices.is_empty());
    Ok(builder.mesh)
}

fn decode_record(store: &RecordStore, rdb_type: u32, id: u32, scene: &mut Scene, object_space: bool, overrides: &[(u32, u32)], inherited: Option<TextureKey>) -> Result<Option<usize>> {
    let Some(bytes) = store.get(rdb_type, id)? else { return Ok(None) };
    let mesh = decode_archive_with(&bytes, object_space, overrides, inherited, |key| {
        if let std::collections::hash_map::Entry::Vacant(e) = scene.textures.entry(key) {
            if let Some(t) = load_texture(store, key).ok().flatten() {
                e.insert(t);
            }
        }
        scene.textures.contains_key(&key)
    })
    .with_context(|| format!("decoding mesh {rdb_type}/{id}"))?;
    scene.meshes.push(mesh);
    Ok(Some(scene.meshes.len() - 1))
}

/// Every texture a mesh record references (before the load check, in first-use order, deduped); `None` = no record.
pub fn texture_refs(store: &RecordStore, rdb_type: u32, id: u32) -> Result<Option<Vec<TextureKey>>> {
    let Some(bytes) = store.get(rdb_type, id)? else { return Ok(None) };
    let mut refs = Vec::new();
    decode_archive(&bytes, false, |k| {
        if !refs.contains(&k) {
            refs.push(k);
        }
        false
    })
    .with_context(|| format!("decoding mesh {rdb_type}/{id}"))?;
    Ok(Some(refs))
}


/// Row-major 4x4 for row vectors (`v' = v * M`), as used by the original engine.
type Mat = [[f32; 4]; 4];
const IDENTITY_MAT: Mat = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];

fn mul(a: &Mat, b: &Mat) -> Mat {
    let mut o = [[0.0; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            o[i][j] = (0..4).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    o
}

/// `RRefFrame_t::UpdateWorldMatrix` local part without the animation matrix: quaternion rows, uniform scale, translation.
fn base_matrix(n: &Object) -> Mat {
    frame_matrix(
        n.f32s::<4>("local_rot").unwrap_or([0.0, 0.0, 0.0, 1.0]),
        n.f32s::<3>("local_pos").unwrap_or([0.0; 3]),
        n.f32s::<1>("scale").map_or(1.0, |s| s[0]),
    )
}

fn frame_matrix([x, y, z, w]: [f32; 4], p: [f32; 3], s: f32) -> Mat {
    let mut m = [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (z * w + x * y), 2.0 * (x * z - w * y), 0.0],
        [2.0 * (x * y - z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z + w * x), 0.0],
        [2.0 * (w * y + x * z), 2.0 * (y * z - w * x), 1.0 - 2.0 * (x * x + y * y), 0.0],
        [p[0], p[1], p[2], 1.0],
    ];
    for row in m.iter_mut().take(3) {
        for v in row.iter_mut().take(3) {
            *v *= s;
        }
    }
    m
}

/// [`base_matrix`], then `anim_matrix * local` when an animation matrix is present.
fn local_matrix(n: &Object) -> Mat {
    let m = base_matrix(n);
    match n.f32s::<16>("anim_matrix") {
        Some(a) => mul(&[[a[0], a[1], a[2], a[3]], [a[4], a[5], a[6], a[7]], [a[8], a[9], a[10], a[11]], [a[12], a[13], a[14], a[15]]], &m),
        None => m,
    }
}

fn det3(m: &Mat) -> f32 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

/// Everything that makes two `SimpleMesh`es mergeable into one [`Submesh`].
#[derive(Clone, Copy)]
struct MatKey {
    texture: Option<TextureKey>,
    stage1_env_texture: Option<TextureKey>,
    depth_write: bool,
    depth_func: Option<i32>,
    alpha_test: Option<i32>,
    alpha_func: i32,
    fog_enabled: bool,
    lighting_enabled: bool,
    render_priority: Option<i32>,
    render_node: Option<(u32, i32)>,
    render_origin: [f32; 3],
    blend: Blend,
    two_sided: bool,
    color: [f32; 4],
    emissive: [f32; 3],
    glow_mask: bool,
    vegetation: bool,
    specular: [f32; 3],
    shininess: f32,
}

type MatId = (Option<TextureKey>, u8, bool, [u32; 4], [u32; 3], bool, [u32; 3], u32,
    (Option<TextureKey>, bool, Option<i32>, Option<i32>, i32, bool, bool, bool),
    (Option<i32>, Option<(u32, i32)>, [u32; 3]));

impl MatKey {
    /// Hashable identity (`Blend` and `f32` are not `Hash`).
    fn id(&self) -> MatId {
        (self.texture, self.blend as u8, self.two_sided, self.color.map(f32::to_bits), self.emissive.map(f32::to_bits), self.glow_mask, self.specular.map(f32::to_bits), self.shininess.to_bits(),
            (self.stage1_env_texture, self.depth_write, self.depth_func, self.alpha_test, self.alpha_func, self.fog_enabled, self.lighting_enabled, self.vegetation),
            (self.render_priority, self.render_node, self.render_origin.map(f32::to_bits)))
    }
}

/// DisplaySystem's cloned archive visual starts transparent nodes in list 6;
/// successful RVisual::Render10049761 changes other authored lists to 3.
fn render_node(ar: &Archive, index: usize, data: &Object) -> (Option<i32>, Option<(u32, i32)>) {
    let transparent = data.refs("mesh").into_iter().filter_map(|i| ar.objects.get(i))
        .filter_map(|sm| sm.ref1("material")).filter_map(|i| ar.objects.get(i))
        .filter_map(|mat| mat.ref1("delta_state")).filter_map(|i| ar.objects.get(i))
        .any(|ds| ds.all("rst_type").flat_map(|b| b.as_chunks::<4>().0.iter())
            .zip(ds.all("rst_value").flat_map(|b| b.as_chunks::<4>().0.iter()))
            .filter(|(t, _)| le(*t) == Some(D3DRS_ALPHABLENDENABLE))
            .last().is_some_and(|(_, value)| le(value) == Some(1)));
    (Some(if transparent { 6 } else { ar.objects[index].i32("prio").unwrap_or(3) }),
        Some((index as u32, if transparent { 6 } else { 3 })))
}

struct Builder<'a, 'b> {
    ar: &'b Archive<'a>,
    mesh: Mesh,
    /// material -> index into `mesh.submeshes`
    groups: HashMap<MatId, usize>,
    have_texture: &'b mut dyn FnMut(TextureKey) -> bool,
    visited: Vec<bool>,
    /// Ignore every node matrix (vertices as stored).
    object_space: bool,
    /// Statel texture overrides `(SimpleMesh slot, texture id)` and the running slot counter.
    overrides: &'b [(u32, u32)],
    slot: usize,
    /// Diffuse texture bound by the most recently drawn `SimpleMesh` (`RDeltaState`'s texture-stage apply,
    /// randy31 `FUN_1002f275`, only issues `SetTexture` for a channel that is actually authored: a `SimpleMesh`
    /// whose node and material carry no texture channel at all leaves the GPU's current texture bound, it does
    /// not clear it). Seeded from `inherited` (whatever the caller's own last draw bound, e.g. the previous
    /// statel in `playfield::Placer`), updated after every `SimpleMesh` to the texture that ends up effectively
    /// bound (resolved, inherited, or overridden) so a run of channel-less `SimpleMesh`es all reuse the last
    /// real texture instead of resetting to none/white.
    last_tex: Option<TextureKey>,
    /// Material identities reset by DisplaySystem's generic resource loader10061505.
    generic_materials: &'b [usize],
    /// StateBlob is installed before node/material deltas (DS1005c334, Randy1004b199).
    object_states: &'b [(i32, i32)],
}

fn decode_archive(bytes: &[u8], object_space: bool, have_texture: impl FnMut(TextureKey) -> bool) -> Result<Mesh> {
    decode_archive_with(bytes, object_space, &[], None, have_texture)
}

fn decode_archive_with(bytes: &[u8], object_space: bool, overrides: &[(u32, u32)], inherited: Option<TextureKey>, mut have_texture: impl FnMut(TextureKey) -> bool) -> Result<Mesh> {
    let ar = Archive::parse(bytes)?;
    let mut b = Builder {
        ar: &ar,
        mesh: Mesh::default(),
        groups: HashMap::new(),
        have_texture: &mut have_texture,
        visited: vec![false; ar.objects.len()],
        object_space,
        overrides,
        slot: 0,
        last_tex: inherited,
        generic_materials: &[],
        object_states: &[],
    };
    b.node(ar.root, &IDENTITY, 0)?;
    b.mesh.submeshes.retain(|s| !s.indices.is_empty());
    Ok(b.mesh)
}

impl Builder<'_, '_> {
    fn node(&mut self, i: usize, parent: &Mat, depth: usize) -> Result<()> {
        ensure!(depth < MAX_DEPTH, "frame tree too deep");
        let ar = self.ar;
        let n = ar.objects.get(i).with_context(|| format!("dangling object ref {i}"))?;
        ensure!(!std::mem::replace(&mut self.visited[i], true), "cyclic frame tree");
        let world = if self.object_space { IDENTITY_MAT } else { mul(&local_matrix(n), parent) };
        if let Some(data) = n.ref1("data") {
            let data = ar.objects.get(data).context("dangling data ref")?;
            let node_ds = n.ref1("delta_state").and_then(|d| ar.objects.get(d));
            for m in data.refs("mesh") {
                self.simple_mesh(node_ds, ar.objects.get(m).context("dangling mesh ref")?, &world, render_node(ar, i, data))?;
            }
        }
        for c in n.refs("chld") {
            self.node(c, &world, depth + 1)?;
        }
        Ok(())
    }

    /// Material of a `SimpleMesh`. Render states and texture channels come from the node's own
    /// `delta_state` (e.g. the "alpha" states of foliage nodes) and the material's `delta_state`
    /// (applied later by `RViewPort_t::SetMaterial`, so it wins); a texture on the node's state wins
    /// over the material's. Colours: `RViewPort_t::SetMaterial` @1004b199 copies only `opac`, `emis`, `spec * shin_str` and
    /// `shin` into the `_D3DMATERIAL7`; its diffuse / ambient RGB stay at `SetDefaultMaterial`'s white (@1004b61e), so `diff` and
    /// `ambi` do not tint anything here (they reach D3D only through `InitD3DMaterial` @100409c6 for sprites and
    /// per-frame material modifiers).
    fn material(&mut self, node_ds: Option<&Object>, sm: &Object) -> MatKey {
        let objs = &self.ar.objects;
        let mat = sm.ref1("material").and_then(|m| objs.get(m));
        let generic = sm.ref1("material").is_some_and(|i| self.generic_materials.contains(&i));
        let mat_ds = mat.and_then(|m| m.ref1("delta_state")).and_then(|d| objs.get(d));
        let state = |ty: i32| {
            [mat_ds, node_ds].into_iter().flatten().find_map(|ds| {
                ds.all("rst_type").flat_map(|v| v.as_chunks::<4>().0.iter()).zip(ds.all("rst_value").flat_map(|v| v.as_chunks::<4>().0.iter()))
                    .filter(|&(t, _)| le(t) == Some(ty)).last().and_then(|(_, v)| le(v))
            }).or_else(|| self.object_states.iter().rev().find(|&&(t, _)| t == ty).map(|&(_, v)| v))
        };
        let mut tex = None;
        let mut foliage_texture = None;
        for ds in [node_ds, mat_ds].into_iter().flatten() {
            // channel 0 = diffuse; fall back to the first bound channel
            let chans: Vec<_> = ds.all("tch_type").map(le).zip(ds.refs("tch_text")).collect();
            let found = chans.iter().find(|(t, _)| *t == Some(0)).or(chans.first()).and_then(|&(_, t)| {
                let texture = objs.get(t)?;
                let creator = texture.ref1("creator").and_then(|c| objs.get(c))?;
                let key = TextureKey { rdb_type: creator.i32("type")? as u32, id: creator.i32("inst")? as u32 };
                if tex.is_none() && key.rdb_type == OVERRIDE_TEXTURES
                    && texture.blob("name").and_then(|b| std::str::from_utf8(b).ok()).is_some_and(|name| foliage_texture_name(name, key))
                {
                    foliage_texture = Some(key);
                }
                Some(key)
            });
            if tex.is_none() {
                tex = found;
            }
        }
        // `FUN_1002f275` (randy31, RDeltaState's texture-stage apply) only issues `SetTexture` for a channel this
        // delta state actually authors (and resolves): no channel at all, or one whose creator record doesn't
        // decode, leaves the GPU's currently bound diffuse texture untouched, so this submesh draws with whatever
        // the previous `SimpleMesh` (in this archive, or -- via `inherited` -- whatever the caller's own last draw)
        // left bound, not blank/white.
        let tex = tex.filter(|&k| (self.have_texture)(k)).or(self.last_tex);
        self.last_tex = tex;
        // Ordinary RVisual environments are enabled by default (+e1), in stage 1 (Randy1004d18a).
        let stage1_env_texture = mat.and_then(|m| m.ref1("env_texture")).and_then(|i| objs.get(i))
            .and_then(|t| t.ref1("creator")).and_then(|i| objs.get(i))
            .and_then(|c| Some(TextureKey { rdb_type: c.i32("type")? as u32, id: c.i32("inst")? as u32 }))
            .filter(|&key| (self.have_texture)(key));
        // Archive deltas retain the device's ALPHAREF0 / GREATEREQUAL7 defaults.
        let alpha_test = (state(D3DRS_ALPHATESTENABLE) == Some(1)).then(|| state(D3DRS_ALPHAREF).unwrap_or(0));
        let alpha_func = state(D3DRS_ALPHAFUNC).unwrap_or(7);
        let depth_write = state(D3DRS_ZWRITEENABLE) != Some(0);
        let depth_func = state(D3DRS_ZFUNC);
        let fog_enabled = state(D3DRS_FOGENABLE) != Some(0);
        let lighting_enabled = state(D3DRS_LIGHTING) != Some(0);
        let blend = if state(D3DRS_ALPHABLENDENABLE) == Some(1) {
            let src = state(D3DRS_SRCBLEND);
            match (src.unwrap_or(D3DBLEND_SRCALPHA), state(D3DRS_DESTBLEND).unwrap_or(6)) {
                (3, 2) => Blend::SourceColorOne,
                (D3DBLEND_ONE, D3DBLEND_ONE) => Blend::AdditiveOne,
                (D3DBLEND_SRCALPHA, D3DBLEND_ONE) => Blend::Additive,
                (1, 3) => Blend::ZeroSourceColor,
                (9, 3) => Blend::DestinationColorSourceColor,
                (D3DBLEND_ONE, 6) => Blend::PremultipliedAlpha,
                _ => Blend::AlphaBlend,
            }
        } else if state(D3DRS_ALPHATESTENABLE) == Some(1) {
            Blend::AlphaTest
        } else {
            Blend::Opaque
        };
        // The scene contract carries colours as `c^2.2`; the renderer shades in the client's gamma space and inverts that.
        let lin = |c: f32| c.max(0.0).powf(2.2);
        let a = mat.and_then(|m| m.f32s::<1>("opac")).map_or(1.0, |o| o[0]);
        // EMISSIVEMATERIALSOURCE (148) stays at the device default MATERIAL for archive meshes: the emissive is `emis` as is
        // (the luminance test of `FUN_1004ac5f` @1004ac5f only runs for `ConvertToLightmap`).
        let emis = if generic { [0.0; 3] } else { mat.and_then(|m| m.f32s::<3>("emis")).unwrap_or([0.0; 3]) };
        let emissive = emis.map(lin);
        // Mode 5 of `FUN_10040645` (texture-alpha glow): stage 0 COLOROP = ADD, COLORARG1 = TEXTURE|ALPHAREPLICATE.
        let stage0 = |ty: i32| [mat_ds, node_ds].into_iter().flatten().find_map(|ds| tss(ds, 0, ty));
        let glow_mask = blend == Blend::Opaque
            && tex.is_some()
            && stage0(D3DTSS_COLOROP) == Some(D3DTOP_ADD)
            && stage0(D3DTSS_COLORARG1).is_some_and(|v| v & D3DTA_ALPHAREPLICATE != 0);
        // `_D3DMATERIAL7.specular` = spec * shin_str, power = shin (`InitD3DMaterial` @100409c6); only drawn with SPECULARENABLE (29).
        let spec = mat.and_then(|m| m.f32s::<3>("spec")).unwrap_or([0.0; 3]);
        let strength = mat.and_then(|m| m.f32s::<1>("shin_str")).map_or(0.0, |v| v[0]);
        let shininess = mat.and_then(|m| m.f32s::<1>("shin")).map_or(0.0, |v| v[0]);
        let (specular, shininess) = if !generic && state(D3DRS_SPECULARENABLE) == Some(1) { (spec.map(|c| lin(c * strength)), shininess) } else { ([0.0; 3], 0.0) };
        MatKey { texture: tex, stage1_env_texture, blend, depth_write, depth_func, alpha_test, alpha_func, fog_enabled, lighting_enabled,
            render_priority: None, render_node: None, render_origin: [0.0; 3],
            vegetation: tex.is_some() && tex == foliage_texture && matches!(blend, Blend::AlphaTest | Blend::AlphaBlend),
            two_sided: state(D3DRS_CULLMODE) == Some(D3DCULL_NONE), color: [1.0, 1.0, 1.0, a], emissive, glow_mask, specular, shininess }
    }

    fn simple_mesh(&mut self, node_ds: Option<&Object>, sm: &Object, world: &Mat, (priority, node): (Option<i32>, Option<(u32, i32)>)) -> Result<()> {
        let desc = sm.get("vb_desc").context("SimpleMesh without vb_desc")?;
        ensure!(desc.len() >= 16, "short vb_desc");
        let fvf = u32::from_le_bytes(desc[8..12].try_into().unwrap());
        let count = u32::from_le_bytes(desc[12..16].try_into().unwrap()) as usize;
        let verts = sm.blob("vertices").context("SimpleMesh without vertices")?;
        let layout = FvfLayout::new(fvf)?;
        ensure!(verts.len() == count * layout.stride, "vertex blob {} != {count} x {}", verts.len(), layout.stride);
        let tris = sm.ref1("trilist").and_then(|t| self.ar.objects.get(t)).and_then(|t| t.blob("triangles")).context("SimpleMesh without triangle list")?;
        ensure!(tris.len() % 6 == 0, "triangle blob not a multiple of 6 bytes");

        let base = self.mesh.vertices.len() as u32;
        let f = |b: &[u8], o: usize| f32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        for v in verts.chunks_exact(layout.stride) {
            let p = [f(v, 0), f(v, 4), f(v, 8)];
            let n = layout.normal.map_or([0.0, 1.0, 0.0], |o| [f(v, o), f(v, o + 4), f(v, o + 8)]);
            let uv = layout.uv.map_or([0.0, 0.0], |o| [f(v, o), f(v, o + 4)]);
            let xf = |a: [f32; 3], t: f32| -> [f32; 3] { std::array::from_fn(|j| a[0] * world[0][j] + a[1] * world[1][j] + a[2] * world[2][j] + t * world[3][j]) };
            let (mut pos, mut nrm) = (xf(p, 1.0), xf(n, 0.0));
            let len = nrm.iter().map(|c| c * c).sum::<f32>().sqrt();
            if len > 1e-12 {
                nrm.iter_mut().for_each(|c| *c /= len);
            }
            pos[2] = -pos[2];
            nrm[2] = -nrm[2];
            self.mesh.vertices.push(Vertex { pos, normal: nrm, uv, ..Default::default() });
        }

        // The Z flip mirrors the mesh; a node matrix with negative determinant mirrors it back.
        let reverse = det3(world) >= 0.0;
        let mut key = self.material(node_ds, sm);
        key.render_priority = priority;
        key.render_node = node;
        key.render_origin = [world[3][0], world[3][1], -world[3][2]];
        let slot = self.slot;
        self.slot += 1;
        if let Some(&(_, id)) = self.overrides.iter().find(|o| o.0 as usize == slot) {
            let tex = TextureKey { rdb_type: OVERRIDE_TEXTURES, id };
            if (self.have_texture)(tex) {
                key.vegetation &= key.texture == Some(tex);
                key.texture = Some(tex);
                self.last_tex = key.texture;
            }
        }
        // Only adjacent identical draws batch: SimpleMesh order is observable with blending.
        let identity = key.id();
        let sub = if let Some(sub) = self.groups.get(&identity).copied().filter(|&sub| sub + 1 == self.mesh.submeshes.len()) {
            sub
        } else {
            self.mesh.submeshes.push(Submesh { blend: key.blend, depth_write: Some(key.depth_write), depth_func: key.depth_func.map(|v| v as u8),
                alpha_test: key.alpha_test.map(|v| v as u8), alpha_func: key.alpha_func as u8,
                fog_enabled: key.fog_enabled, lighting_enabled: key.lighting_enabled,
                render_priority: key.render_priority, render_node: key.render_node, render_origin: key.render_origin,
                vegetation: key.vegetation,
                stage1_env_texture: key.stage1_env_texture, projection_pass: true, two_sided: key.two_sided, base_color: key.color, emissive: key.emissive,
                glow_mask: key.glow_mask, specular: key.specular, shininess: key.shininess, ..Submesh::new(vec![], key.texture) });
            let sub = self.mesh.submeshes.len() - 1;
            self.groups.insert(identity, sub);
            sub
        };
        for t in tris.as_chunks::<6>().0 {
            let ix: [u32; 3] = std::array::from_fn(|k| u16::from_le_bytes([t[2 * k], t[2 * k + 1]]) as u32);
            if ix.iter().any(|&i| i as usize >= count) {
                bail!("triangle index out of range (vertex count {count})");
            }
            let [a, b, c] = ix.map(|i| base + i);
            self.mesh.submeshes[sub].indices.extend(if reverse { [a, c, b] } else { [a, b, c] });
        }
        Ok(())
    }
}

/// Texture-stage state `ty` of `stage` in a `RDeltaState` (`tstm_count` entries per stage, then
/// `tst_type`/`tst_value` pairs, each member holding all elements; last write wins).
fn tss(ds: &Object, stage: usize, ty: i32) -> Option<i32> {
    let ints = |n: &str| ds.all(n).flat_map(|d| d.as_chunks::<4>().0.iter()).map(|c| i32::from_le_bytes(*c)).collect::<Vec<_>>();
    let (counts, types, values) = (ints("tstm_count"), ints("tst_type"), ints("tst_value"));
    let first: usize = counts.get(..stage)?.iter().map(|&c| c as usize).sum();
    let n = *counts.get(stage)? as usize;
    types.get(first..first + n)?.iter().zip(values.get(first..first + n)?).rfind(|(t, _)| **t == ty).map(|(_, v)| *v)
}

fn le(d: &[u8]) -> Option<i32> {
    d.get(..4).map(|b| i32::from_le_bytes(b.try_into().unwrap()))
}

/// Byte offsets inside one D3D9 FVF vertex (the data uses 0x112 = XYZ | NORMAL | TEX1 everywhere).
struct FvfLayout {
    stride: usize,
    normal: Option<usize>,
    uv: Option<usize>,
}

impl FvfLayout {
    fn new(fvf: u32) -> Result<Self> {
        ensure!(fvf & 0xe == 0x2, "unsupported FVF {fvf:#x} (position must be XYZ)");
        let mut o = 12;
        let normal = (fvf & 0x10 != 0).then_some(o);
        if normal.is_some() {
            o += 12;
        }
        o += 4 * ((fvf >> 6) & 1) as usize + 4 * ((fvf >> 7) & 1) as usize; // diffuse, specular
        let tex = ((fvf >> 8) & 0xf) as usize;
        let uv = (tex > 0).then_some(o);
        Ok(FvfLayout { stride: o + 8 * tex, normal, uv })
    }
}

/// The `RLight_t` nodes of a mesh archive (`light_info` = a `D3DLIGHT7`, `RLight_t::RLight_t(ObjectArchive_c*)` randy31
/// @0x1003fe81) as renderer-space lights (Z negated, `offset` added in AO space first). `RLight_t::Process` (@0x1003ff72)
/// registers every light of the loaded frame tree; `RVisual_t::CullLights` (@0x1004ce96) hands them to each visual that shares
/// a `grp_mask` bit (all ones by default, also in the data), without any range test while at most 8 exist, so the list is
/// what lights the meshes. Colours are `diffuse^2.2` like the statel lights; lights whose diffuse colour is black add
/// nothing and are dropped. Directional lights (`D3DLIGHT_DIRECTIONAL`) do not occur in the data and are rejected.
pub fn decode_mesh_lights(store: &RecordStore, rdb_type: u32, id: u32, offset: [f32; 3]) -> Result<Vec<ao_scene::Light>> {
    let Some(bytes) = store.get(rdb_type, id)? else { return Ok(vec![]) };
    let ar = Archive::parse(&bytes)?;
    let mut out = vec![];
    let mut visited = vec![false; ar.objects.len()];
    let mut stack = vec![(ar.root, IDENTITY_MAT, 0usize)];
    while let Some((i, parent, depth)) = stack.pop() {
        ensure!(depth < MAX_DEPTH, "frame tree too deep");
        let n = ar.objects.get(i).with_context(|| format!("dangling object ref {i}"))?;
        ensure!(!std::mem::replace(&mut visited[i], true), "cyclic frame tree");
        let world = mul(&local_matrix(n), &parent);
        if let Some(info) = n.get("light_info") {
            ensure!(info.len() == 0x68, "light_info of {} bytes", info.len());
            let w = |k: usize| u32::from_le_bytes(info[4 * k..4 * k + 4].try_into().unwrap());
            let f = |k: usize| f32::from_bits(w(k));
            // D3DLIGHT7: type, diffuse rgba, specular rgba, ambient rgba, position, direction, range, falloff, att0..2, theta, phi
            let diffuse = [f(1), f(2), f(3)];
            if diffuse != [0.0; 3] {
                let scene = |v: [f32; 3]| [v[0] + offset[0], v[1] + offset[1], -(v[2] + offset[2])];
                let spot = match w(0) {
                    1 => None,
                    2 => Some(ao_scene::Spot { dir: [world[2][0], world[2][1], -world[2][2]], theta: f(24), phi: f(25) }),
                    t => bail!("RLight_t of D3D type {t} in mesh {id}"),
                };
                let att = [f(21), f(22), f(23)];
                out.push(ao_scene::Light {
                    pos: scene([world[3][0], world[3][1], world[3][2]]),
                    color: diffuse.map(|c| c.powf(2.2)),
                    range: f(19),
                    // all zero attenuation is 1/0 in D3D (full intensity), distinct from the contract's "linear" marker
                    atten: if att == [0.0; 3] { [f32::MIN_POSITIVE, 0.0, 0.0] } else { att },
                    spot,
                    zone: None,
                    owner: None,
                });
            }
        }
        stack.extend(n.refs("chld").into_iter().map(|c| (c, world, depth + 1)));
    }
    Ok(out)
}

/// The named connectors of a mesh archive (`RConnector_t`, class 0x4b: `name` + `originator` = the frame node it hangs
/// on; `VisualMesh_t::GetConnector` / `CCCharacter_t::MoveToConnector`) with the node's world matrix, **AO space**
/// (row-vector convention, left-handed, no Z mirror). `anim_matrix * local` of every ancestor applies as for the meshes.
pub fn decode_mesh_connectors(store: &RecordStore, rdb_type: u32, id: u32) -> Result<Vec<(String, [[f32; 4]; 4])>> {
    let Some(bytes) = store.get(rdb_type, id)? else { return Ok(vec![]) };
    let ar = Archive::parse(&bytes)?;
    let mut world: Vec<Option<Mat>> = vec![None; ar.objects.len()];
    let mut stack = vec![(ar.root, IDENTITY_MAT, 0usize)];
    while let Some((i, parent, depth)) = stack.pop() {
        ensure!(depth < MAX_DEPTH, "frame tree too deep");
        let n = ar.objects.get(i).with_context(|| format!("dangling object ref {i}"))?;
        let w = mul(&local_matrix(n), &parent);
        world[i] = Some(w);
        stack.extend(n.refs("chld").into_iter().map(|c| (c, w, depth + 1)));
    }
    Ok(ar
        .objects
        .iter()
        .filter_map(|o| {
            let name = String::from_utf8_lossy(o.blob("name")?).into_owned();
            let node = usize::try_from(o.i32("originator")?).ok()?;
            Some((name, (*world.get(node)?)?))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vegetation_requires_native_named_foliage_and_nonopaque_material() {
        fn classify(name: &str, texture_type: u32, alpha: bool, loaded: bool) -> bool {
            use crate::archive::Member;
            fn m<'a>(name: &'a str, data: &'a [u8]) -> Member<'a> { Member { name, data } }
            let one = 1u32.to_le_bytes();
            let two = 2u32.to_le_bytes();
            let three = 3u32.to_le_bytes();
            let four = 4u32.to_le_bytes();
            let zero = 0u32.to_le_bytes();
            let texture_type = texture_type.to_le_bytes();
            let alpha_state = D3DRS_ALPHATESTENABLE.to_le_bytes();
            let alpha_value = u32::from(alpha).to_le_bytes();
            let name = blob(name.as_bytes());
            let ar = Archive { root: 0, animation_ranges: vec![], objects: vec![
                Object { members: vec![m("material", &one)] },
                Object { members: vec![m("delta_state", &two)] },
                Object { members: vec![m("tch_type", &zero), m("tch_text", &three),
                    m("rst_type", &alpha_state), m("rst_value", &alpha_value)] },
                Object { members: vec![m("name", &name), m("creator", &four)] },
                Object { members: vec![m("type", &texture_type), m("inst", &one)] },
            ] };
            let mut have_texture = |_| loaded;
            let mut builder = Builder { ar: &ar, mesh: Mesh::default(), groups: HashMap::new(),
                have_texture: &mut have_texture, visited: vec![], object_space: false,
                overrides: &[], slot: 0, last_tex: None, generic_materials: &[], object_states: &[] };
            builder.material(None, &ar.objects[0]).vegetation
        }
        for name in ["leaves_hires", "applestyletrees_newleaves", "grass_medium_high",
            "plant_billboards_1024_lars2", "palmeblad"] {
            assert!(classify(name, OVERRIDE_TEXTURES, true, true), "{name}");
            assert!(!classify(name, OVERRIDE_TEXTURES, false, true), "opaque {name}");
            assert!(!classify(name, OVERRIDE_TEXTURES, true, false), "missing {name}");
            assert!(!classify(name, 1_010_011, true, true), "CAT texture {name}");
        }
        for name in ["bark", "plant_trunk_001", "inferno_frozen_jungletrees_trunk",
            "barkgrasspart", "grass_texture_tiles256", "body_leaves", "arms_fern_tshirt",
            "bannerwork_unredeemed_inferno2", "hq_implant_sign", "cloak", "unknown"] {
            assert!(!classify(name, OVERRIDE_TEXTURES, true, true), "{name}");
        }
        assert!(!Submesh::new(vec![], None).vegetation);
        assert!(foliage_texture_name("branch", TextureKey { rdb_type: OVERRIDE_TEXTURES, id: 257704 }));
        assert!(!foliage_texture_name("branch", TextureKey { rdb_type: OVERRIDE_TEXTURES, id: 1 }));
    }

    #[test]
    fn installed_borealis_brown_canopy_retains_foliage_but_not_overridden_trunk() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let dir = std::path::PathBuf::from(home).join("Games/ProjectRubiKa/client");
        let Ok(store) = RecordStore::open(&dir) else { return };
        let names = crate::character::NameTable::load(&store).unwrap();
        let mesh = names.id(MESH_TYPE, "large-brown-tree1.abiff").unwrap();
        let canopy = TextureKey { rdb_type: OVERRIDE_TEXTURES, id: names.id(OVERRIDE_TEXTURES, "branch.png").unwrap() };
        let bark = TextureKey { rdb_type: OVERRIDE_TEXTURES, id: names.id(OVERRIDE_TEXTURES, "spookybark.png").unwrap() };
        // Actual camera-target placement: rdb1000003/800 +0x267a, AO(651.82959,65.42179,428.47311).
        let placement = store.get(1_000_003, 800).unwrap().unwrap();
        assert_eq!(u32::from_le_bytes(placement[0x268a..0x268e].try_into().unwrap()), mesh);
        assert_eq!(u32::from_le_bytes(placement[0x2694..0x2698].try_into().unwrap()), bark.id);
        for ty in [MESH_TYPE, MESH_LOW_TYPE] {
            let mut scene = Scene::default();
            let index = decode_statel_mesh(&store, ty, mesh, &[(0, bark.id)], None, &mut scene).unwrap().unwrap();
            let mesh = &scene.meshes[index];
            let leaves: Vec<_> = mesh.submeshes.iter().filter(|s| s.texture == Some(canopy)).collect();
            assert!(!leaves.is_empty(), "native canopy missing in family {ty}");
            for s in leaves {
                assert!(s.vegetation && !s.indices.is_empty(), "native autumn canopy family {ty}");
                // Classification must retain the native blend/test/culling, not rewrite it for sway.
                assert_eq!(s.blend, Blend::AlphaBlend);
                assert_eq!((s.alpha_test, s.alpha_func, s.two_sided), (Some(128), 5, true));
            }
            let trunks: Vec<_> = mesh.submeshes.iter().filter(|s| s.texture == Some(bark)).collect();
            assert!(!trunks.is_empty(), "placement bark override missing in family {ty}");
            assert!(trunks.iter().all(|s| !s.vegetation));
        }
    }

    #[test]
    fn generic_visual_uses_factory_frame_or_selected_mesh_frame() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let dir = std::path::PathBuf::from(home).join("Games/ProjectRubiKa/client");
        let Ok(store) = RecordStore::open(&dir) else { return };
        let names = crate::character::NameTable::load(&store).unwrap();
        let rock = store.get(MESH_TYPE, 36067).unwrap().expect("rock01");
        let rock = Archive::parse(&rock).unwrap();
        let default = rock.objects.iter().find(|o| o.get("anim_rot").is_some()).unwrap();
        let inherited = mesh_animation_matrix(default).unwrap();
        let close = |a: [f32; 3], b: [f32; 3]| {
            assert!((0..3).all(|k| (a[k] - b[k]).abs() < 0.001), "{a:?} != {b:?}");
        };
        for name in ["skydome_clouds.abiff", "horizon_object.abiff"] {
            let id = names.id(MESH_TYPE, name).expect(name);
            let bytes = store.get(MESH_TYPE, id).unwrap().unwrap();
            let ar = Archive::parse(&bytes).unwrap();
            let data = ar.objects[ar.root].ref1("data").expect("single mesh root");
            let native = mesh_animation_matrix(&ar.objects[data]).unwrap();
            let raw = decode_archive(&bytes, true, |_| false).unwrap();
            let world = generic_mesh_archive(&ar, default, None, false, &[], &mut |_| false).unwrap();
            let locked = generic_mesh_archive(&ar, default, None, true, &[], &mut |_| false).unwrap();
            let selected = generic_mesh_archive(&ar, default, Some(0), false, &[], &mut |_| false).unwrap();
            assert_eq!(raw.vertices.len(), world.vertices.len());
            assert_eq!(world.submeshes[0].indices, raw.submeshes[0].indices);
            for (i, v) in raw.vertices.iter().enumerate() {
                let p = [v.pos[0], v.pos[1], -v.pos[2]];
                let n = [v.normal[0], v.normal[1], -v.normal[2]];
                let xf = |p: [f32; 3], m: &Mat, translation: bool| {
                    let mut out: [f32; 3] = std::array::from_fn(|j| (0..3).map(|k| p[k] * m[k][j]).sum::<f32>() + if translation { m[3][j] } else { 0.0 });
                    out[2] = -out[2];
                    out
                };
                close(world.vertices[i].pos, xf(p, &inherited, true));
                close(locked.vertices[i].pos, xf(p, &inherited, false));
                close(selected.vertices[i].pos, xf(p, &native, true));
                let mut normal = xf(n, &inherited, false);
                let length = normal.iter().map(|v| v * v).sum::<f32>().sqrt();
                normal.iter_mut().for_each(|v| *v /= length);
                close(locked.vertices[i].normal, normal);
            }
            assert!(generic_mesh_archive(&ar, default, Some(usize::MAX), false, &[], &mut |_| false).is_err());
        }
    }

    /// Loading textures must not replace native blending with an alpha test, even for binary alpha.
    #[test]
    fn binary_alpha_texture_keeps_archived_blending() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let dir = std::path::PathBuf::from(home).join("Games/ProjectRubiKa/client");
        let Ok(store) = RecordStore::open(&dir) else { return };
        for id in store.ids(MESH_TYPE).unwrap() {
            let bytes = store.get(MESH_TYPE, id).unwrap().unwrap();
            let native = decode_archive(&bytes, false, |_| true).unwrap();
            let mut scene = Scene::default();
            let Some(index) = decode_mesh_into(&store, id, &mut scene).unwrap() else { continue };
            for (native, loaded) in native.submeshes.iter().zip(&scene.meshes[index].submeshes) {
                if native.blend != Blend::AlphaBlend || native.base_color[3] < 1.0 { continue; }
                let Some(texture) = loaded.texture.and_then(|key| scene.textures.get(&key)) else { continue };
                if texture.rgba.as_chunks::<4>().0.iter().all(|pixel| matches!(pixel[3], 0 | 255)) {
                    assert_eq!(loaded.blend, Blend::AlphaBlend, "binary-alpha mesh {id}");
                    return;
                }
            }
        }
        panic!("client has no binary-alpha blended mesh");
    }

    #[test]
    fn native_environment_and_node_priority_reach_scene_materials() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let dir = std::path::PathBuf::from(home).join("Games/ProjectRubiKa/client");
        let Ok(store) = RecordStore::open(&dir) else { return };
        let env = load_mesh(&store, 29086).unwrap();
        assert!(env.meshes[0].submeshes.iter().any(|s| s.stage1_env_texture.is_some()));
        assert!(env.meshes[0].submeshes.iter().all(|s| s.env_texture.is_none()), "ordinary env is not the CAT redraw");
        assert!(env.meshes[0].submeshes.iter().all(|s| s.projection_pass), "archive RTriMesh provenance is independent of render_node/skin buffers");
        for (id, priority) in [(1414, 3), (1436, 6), (206280, 6)] {
            let scene = load_mesh(&store, id).unwrap();
            assert!(scene.meshes[0].submeshes.iter().any(|s| s.render_priority == Some(priority)), "1010001/{id} priority {priority}");
        }
    }


    #[test]
    fn generic_resource_resets_raw_cloud_material_emission_and_specular() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let dir = std::path::PathBuf::from(home).join("Games/ProjectRubiKa/client");
        let Ok(store) = RecordStore::open(&dir) else { return };
        for id in [227651, 227745] {
            let bytes = store.get(MESH_TYPE, id).unwrap().unwrap();
            let raw = decode_archive(&bytes, true, |_| false).unwrap();
            assert_eq!(raw.submeshes[0].emissive, [1.0; 3], "raw archive {id} really authors emissive1");
            let mut scene = Scene::default();
            let decoded = decode_generic_mesh(&store, id, None, true, &[], &mut scene).unwrap().unwrap();
            for sub in &scene.meshes[decoded].submeshes {
                assert_eq!(sub.emissive, [0.0; 3], "10061505 clears first material emission");
                assert_eq!(sub.specular, [0.0; 3]);
                assert_eq!(sub.base_color[..3], [1.0; 3]);
            }
        }
    }

    #[test]
    fn generic_material_reset_follows_shared_identity_not_submesh_slot() {
        use crate::archive::Member;
        let one = [1usize, 3].map(|i| (i as u32).to_le_bytes());
        let rgb: Vec<u8> = [0.5f32; 3].into_iter().flat_map(f32::to_le_bytes).collect();
        let scalar = 1.0f32.to_le_bytes();
        let state = 5u32.to_le_bytes();
        let material = || Object { members: vec![
            Member { name: "emis", data: &rgb }, Member { name: "spec", data: &rgb },
            Member { name: "shin_str", data: &scalar }, Member { name: "shin", data: &scalar },
            Member { name: "delta_state", data: &state },
        ] };
        let mesh = |i: usize| Object { members: vec![Member { name: "material", data: &one[i][..] }] };
        let rs = D3DRS_SPECULARENABLE.to_le_bytes();
        let enabled = 1i32.to_le_bytes();
        let ar = Archive { root: 0, animation_ranges: vec![], objects: vec![
            mesh(0), material(), mesh(1), material(), mesh(0),
            Object { members: vec![Member { name: "rst_type", data: &rs }, Member { name: "rst_value", data: &enabled }] },
        ] };
        let mut textures = |_| false;
        let mut builder = Builder { ar: &ar, mesh: Mesh::default(), groups: HashMap::new(), have_texture: &mut textures,
            visited: vec![], object_space: true, overrides: &[], slot: 0, last_tex: None, generic_materials: &[1], object_states: &[] };
        for i in [0, 4] {
            let material = builder.material(None, &ar.objects[i]);
            assert_eq!(material.emissive, [0.0; 3]);
            assert_eq!(material.specular, [0.0; 3]);
        }
        let other = builder.material(None, &ar.objects[2]);
        assert_eq!(other.emissive, [0.5f32.powf(2.2); 3], "another material identity must retain its authored properties");
        assert_eq!(other.specular, [0.5f32.powf(2.2); 3]);
    }

    #[test]
    fn packed_material_states_override_node_and_object_before_batching() {
        use crate::archive::Member;
        let material = 1u32.to_le_bytes();
        let state = 2u32.to_le_bytes();
        let types: Vec<u8> = [D3DRS_ALPHABLENDENABLE, D3DRS_CULLMODE].into_iter().flat_map(i32::to_le_bytes).collect();
        let values: Vec<u8> = [1i32, D3DCULL_NONE].into_iter().flat_map(i32::to_le_bytes).collect();
        let disabled = 0i32.to_le_bytes();
        let alpha = D3DRS_ALPHABLENDENABLE.to_le_bytes();
        let node = Object { members: vec![Member { name: "rst_type", data: &alpha }, Member { name: "rst_value", data: &disabled }] };
        let ar = Archive { root: 0, animation_ranges: vec![], objects: vec![
            Object { members: vec![Member { name: "material", data: &material }] },
            Object { members: vec![Member { name: "delta_state", data: &state }] },
            Object { members: vec![Member { name: "rst_type", data: &types }, Member { name: "rst_value", data: &values }] },
        ] };
        let mut textures = |_| false;
        let mut builder = Builder { ar: &ar, mesh: Mesh::default(), groups: HashMap::new(), have_texture: &mut textures,
            visited: vec![], object_space: true, overrides: &[], slot: 0, last_tex: None, generic_materials: &[],
            object_states: &[(D3DRS_ALPHABLENDENABLE, 0), (D3DRS_CULLMODE, 2), (D3DRS_SRCBLEND, 3), (D3DRS_DESTBLEND, 2)] };
        let effective = builder.material(Some(&node), &ar.objects[0]);
        assert_eq!(effective.blend, Blend::SourceColorOne, "material enables blending but inherits object blend factors");
        assert!(effective.two_sided, "second packed material state overrides object culling");
        let without_material = Object { members: vec![] };
        builder.object_states = &[(D3DRS_ALPHABLENDENABLE, 1)];
        assert_eq!(builder.material(Some(&node), &without_material).blend, Blend::Opaque, "node overrides the object");
    }

    #[test]
    fn fvf_0x112_is_32_bytes() {
        let l = FvfLayout::new(0x112).unwrap();
        assert_eq!((l.stride, l.normal, l.uv), (32, Some(12), Some(24)));
    }

    #[test]
    fn identity_node_matrix() {
        let m = local_matrix(&Object { members: vec![] });
        assert_eq!(m, IDENTITY);
    }

    #[test]
    fn quat_90_about_x_rotates_y_to_z() {
        let q: Vec<u8> = [std::f32::consts::FRAC_1_SQRT_2, 0.0, 0.0, std::f32::consts::FRAC_1_SQRT_2].iter().flat_map(|f| f.to_le_bytes()).collect();
        let n = Object { members: vec![crate::archive::Member { name: "local_rot", data: &q }] };
        let m = local_matrix(&n);
        // row-vector convention: (0,1,0) * M = row 1 = (0,0,1)
        assert!((m[1][2] - 1.0).abs() < 1e-5 && m[1][1].abs() < 1e-5);
    }

    fn member(idx: u8, ty: u32, data: &[u8]) -> Vec<u8> {
        let mut v = vec![idx];
        for x in [ty, 0, data.len() as u32] {
            v.extend(x.to_le_bytes());
        }
        v.extend(data);
        v
    }
    fn object(members: &[Vec<u8>]) -> Vec<u8> {
        let mut v = vec![];
        for x in [1u32, 1, members.len() as u32] {
            v.extend(x.to_le_bytes());
        }
        members.concat().into_iter().for_each(|b| v.push(b));
        v
    }
    fn blob(b: &[u8]) -> Vec<u8> {
        [&(b.len() as u32).to_le_bytes()[..], b].concat()
    }

    /// One quad as stored in record 1010001/214381 (Box02 ground plane: x/y in +-25, normal -Z,
    /// rotated to +Y by its anim_matrix), no material.
    #[test]
    fn decodes_quad_node_with_anim_matrix() {
        const NAMES: [&str; 8] = ["obj", "anim_matrix", "data", "mesh", "vb_desc", "vertices", "trilist", "triangles"];
        let mut b = vec![];
        for x in [3u32, 0, 0, 1, NAMES.len() as u32] {
            b.extend(x.to_le_bytes());
        }
        for n in NAMES {
            b.extend(format!("1\0{n}\0").bytes());
        }
        b.extend(4u32.to_le_bytes()); // object count
        let fl = |v: &[f32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
        let r = |v: &[i32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
        b.extend(object(&[member(0, 0x11, &r(&[0]))]));
        // Rx(+90deg) in row-vector form: y -> z, z -> -y, plus translation (1, 2, 3)
        let am = fl(&[1., 0., 0., 0., 0., 0., 1., 0., 0., -1., 0., 0., 1., 2., 3., 1.]);
        b.extend(object(&[member(1, 0xf, &am), member(2, 0x11, &r(&[1]))]));
        b.extend(object(&[member(3, 0x11, &r(&[2]))]));
        let quad = [(-25., 25.), (25., 25.), (-25., -25.), (25., -25.)];
        let vb: Vec<f32> = quad.iter().flat_map(|&(x, y)| [x, y, 0., 0., 0., -1., 0.5, 0.5]).collect();
        let mut desc = vec![];
        for x in [16u32, 65536, 0x112, 4] {
            desc.extend(x.to_le_bytes());
        }
        b.extend(object(&[member(4, 9, &desc), member(5, 9, &blob(&fl(&vb))), member(6, 0x11, &r(&[3]))]));
        let tris: Vec<u8> = [0u16, 1, 2, 3, 2, 1].iter().flat_map(|i| i.to_le_bytes()).collect();
        b.extend(object(&[member(7, 9, &blob(&tris))]));
        b.extend([0u8; 12]);

        let m = decode_archive(&b, false, |_| false).unwrap();
        assert_eq!(m.vertices.len(), 4);
        // (-25, 25, 0) * M + t = (-24, 2, 28); Z negated for the right-handed output -> -28
        assert_eq!(m.vertices[0].pos, [-24.0, 2.0, -28.0]);
        // normal (0,0,-1) -> (0, 1, 0) after Rx, then Z flip leaves Y untouched
        let n = m.vertices[0].normal;
        assert!((n[1] - 1.0).abs() < 1e-6 && n[0].abs() < 1e-6 && n[2].abs() < 1e-6);
        // winding reversed by the handedness flip
        assert_eq!(m.submeshes.len(), 1);
        assert_eq!(m.submeshes[0].indices, vec![0, 2, 1, 3, 1, 2]);
        assert!(m.submeshes[0].texture.is_none());
        assert!(m.submeshes[0].projection_pass);
    }

    /// Two untextured `SimpleMesh`es (one triangle each) that merge into one submesh; a statel texture override on
    /// slot 1 must split the second one off with the override texture (`NewTextureData_t`, see `decode_statel_mesh`).
    #[test]
    fn statel_override_replaces_the_texture_of_one_slot() {
        const NAMES: [&str; 7] = ["obj", "data", "mesh", "vb_desc", "vertices", "trilist", "triangles"];
        let mut b = vec![];
        for x in [3u32, 0, 0, 1, NAMES.len() as u32] {
            b.extend(x.to_le_bytes());
        }
        for n in NAMES {
            b.extend(format!("1\0{n}\0").bytes());
        }
        b.extend(6u32.to_le_bytes());
        let fl = |v: &[f32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
        let r = |v: &[i32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
        b.extend(object(&[member(0, 0x11, &r(&[0]))]));
        b.extend(object(&[member(1, 0x11, &r(&[1]))]));
        b.extend(object(&[member(2, 0x11, &r(&[2, 3]))]));
        let vb = fl(&[0., 0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 0., 1., 0., 1., 0., 0., 1., 0., 1., 0., 0., 1., 1.]);
        let mut desc = vec![];
        for x in [16u32, 65536, 0x112, 3] {
            desc.extend(x.to_le_bytes());
        }
        for tri in [4, 5] {
            b.extend(object(&[member(3, 9, &desc), member(4, 9, &blob(&vb)), member(5, 0x11, &r(&[tri]))]));
        }
        let tris: Vec<u8> = [0u16, 1, 2].iter().flat_map(|i| i.to_le_bytes()).collect();
        for _ in 0..2 {
            b.extend(object(&[member(6, 9, &blob(&tris))]));
        }
        b.extend([0u8; 12]);

        assert_eq!(decode_archive(&b, false, |_| true).unwrap().submeshes.len(), 1);
        let key = TextureKey { rdb_type: OVERRIDE_TEXTURES, id: 77 };
        let m = decode_archive_with(&b, false, &[(1, 77)], None, |k| k == key).unwrap();
        assert_eq!(m.submeshes.len(), 2);
        assert_eq!((m.submeshes[0].texture, m.submeshes[1].texture), (None, Some(key)));
        // an override whose texture does not exist leaves the slot as authored
        assert_eq!(decode_archive_with(&b, false, &[(1, 77)], None, |_| false).unwrap().submeshes.len(), 1);
    }

    /// `RDeltaState::FUN_1002f275` (randy31) never clears texture stage 0 for a channel-less `SimpleMesh`: a bare
    /// mesh with no material/texture channel at all draws with whatever the caller passes as `inherited` (what the
    /// native renderer's previous draw call left bound), not a blank/white fallback.
    #[test]
    fn textureless_simple_mesh_inherits_the_caller_supplied_texture() {
        const NAMES: [&str; 7] = ["obj", "data", "mesh", "vb_desc", "vertices", "trilist", "triangles"];
        let mut b = vec![];
        for x in [3u32, 0, 0, 1, NAMES.len() as u32] {
            b.extend(x.to_le_bytes());
        }
        for n in NAMES {
            b.extend(format!("1\0{n}\0").bytes());
        }
        b.extend(4u32.to_le_bytes());
        let fl = |v: &[f32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
        let r = |v: &[i32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
        b.extend(object(&[member(0, 0x11, &r(&[0]))]));
        b.extend(object(&[member(1, 0x11, &r(&[1]))]));
        b.extend(object(&[member(2, 0x11, &r(&[2]))]));
        let vb = fl(&[0., 0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 0., 1., 0., 1., 0., 0., 1., 0., 1., 0., 0., 1., 1.]);
        let mut desc = vec![];
        for x in [16u32, 65536, 0x112, 3] {
            desc.extend(x.to_le_bytes());
        }
        b.extend(object(&[member(3, 9, &desc), member(4, 9, &blob(&vb)), member(5, 0x11, &r(&[3]))]));
        let tris: Vec<u8> = [0u16, 1, 2].iter().flat_map(|i| i.to_le_bytes()).collect();
        b.extend(object(&[member(6, 9, &blob(&tris))]));
        b.extend([0u8; 12]);

        // no `inherited` seed: the existing, unaffected behaviour (nothing bound before this mesh's own draw).
        assert_eq!(decode_archive(&b, false, |_| false).unwrap().submeshes[0].texture, None);
        // seeded: this archive has no texture channel of its own anywhere, so the whole mesh draws with
        // whatever the caller says was already bound.
        let key = TextureKey { rdb_type: 1_010_004, id: 9001 };
        let m = decode_archive_with(&b, false, &[], Some(key), |_| false).unwrap();
        assert_eq!(m.submeshes[0].texture, Some(key));
    }

    #[test]
    fn out_of_range_child_ref_is_an_error() {
        let mut b = vec![];
        for x in [3u32, 0, 0, 1, 2] {
            b.extend(x.to_le_bytes());
        }
        for n in ["obj", "chld"] {
            b.extend(format!("1\0{n}\0").bytes());
        }
        b.extend(1u32.to_le_bytes());
        let r = |v: &[i32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
        b.extend(object(&[member(0, 0x11, &r(&[0]))]));
        b.extend(object(&[member(1, 0x11, &r(&[0x4d00_0000]))]));
        b.extend([0u8; 12]);
        assert!(decode_archive(&b, false, |_| false).is_err());
    }

    /// One triangle with a `FAFMaterial_t` (diffuse, opacity) and a material `RDeltaState` holding `states`.
    fn material_mesh(diff: [f32; 3], opac: f32, states: &[(i32, i32)]) -> Submesh {
        material_mesh_emis(diff, [0.0; 3], opac, states)
    }

    fn material_mesh_emis(diff: [f32; 3], emis: [f32; 3], opac: f32, states: &[(i32, i32)]) -> Submesh {
        const NAMES: [&str; 14] = ["obj", "data", "mesh", "vb_desc", "vertices", "trilist", "triangles", "material", "delta_state", "diff", "opac", "rst_type", "rst_value", "emis"];
        let mut b = vec![];
        for x in [3u32, 0, 0, 1, NAMES.len() as u32] {
            b.extend(x.to_le_bytes());
        }
        for n in NAMES {
            b.extend(format!("1\0{n}\0").bytes());
        }
        b.extend(6u32.to_le_bytes());
        let fl = |v: &[f32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
        let r = |v: &[i32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
        b.extend(object(&[member(0, 0x11, &r(&[0]))])); // holder -> root 0 (the node)
        b.extend(object(&[member(1, 0x11, &r(&[1]))])); // node: data -> 1
        b.extend(object(&[member(2, 0x11, &r(&[2]))])); // data: mesh -> 2
        let vb = fl(&[0., 0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 1., 0., 1., 0., 0., 1., 0., 0., 1.]);
        let mut desc = vec![];
        for x in [16u32, 65536, 0x112, 3] {
            desc.extend(x.to_le_bytes());
        }
        b.extend(object(&[member(3, 9, &desc), member(4, 9, &blob(&vb)), member(5, 0x11, &r(&[3])), member(7, 0x11, &r(&[4]))]));
        let tris: Vec<u8> = [0u16, 1, 2].iter().flat_map(|i| i.to_le_bytes()).collect();
        b.extend(object(&[member(6, 9, &blob(&tris))]));
        b.extend(object(&[member(8, 0x11, &r(&[5])), member(9, 0x10, &fl(&diff)), member(10, 0xa, &fl(&[opac])), member(13, 0x10, &fl(&emis))]));
        let mut ds = vec![];
        for &(t, v) in states {
            ds.push(member(11, 3, &r(&[t])));
            ds.push(member(12, 3, &r(&[v])));
        }
        b.extend(object(&ds));
        b.extend([0u8; 12]);
        let mut m = decode_archive(&b, false, |_| false).unwrap();
        assert_eq!(m.submeshes.len(), 1);
        m.submeshes.remove(0)
    }

    #[test]
    fn diffuse_is_white_opacity_is_kept_and_default_is_opaque_culled() {
        // `diff` never reaches the D3D material of an archive mesh (RViewPort_t::SetMaterial), `opac` does
        let s = material_mesh([0.5, 0.0, 1.0], 0.25, &[]);
        assert_eq!((s.blend, s.two_sided, s.texture), (Blend::Opaque, false, None));
        assert_eq!(s.base_color, [1.0, 1.0, 1.0, 0.25]);
    }

    #[test]
    fn emissive_is_the_material_emis_in_contract_colours() {
        let s = material_mesh_emis([1.0; 3], [0.5, 0.0, 1.0], 1.0, &[]);
        assert!((s.emissive[0] - 0.5_f32.powf(2.2)).abs() < 1e-6 && s.emissive[1] == 0.0 && s.emissive[2] == 1.0);
        assert_eq!(material_mesh([1.0; 3], 1.0, &[]).emissive, [0.0; 3]);
        assert!(!s.glow_mask);
    }

    #[test]
    fn texture_stage_states_are_read_per_stage() {
        // mode 5 of @10040645: stage 0 {ARG1 = TEXTURE|ALPHAREPLICATE, OP = ADD, ARG2 = DIFFUSE, ALPHAOP = DISABLE}, stage 1 {ARG1 = TEXTURE, OP = MODULATE, ARG2 = CURRENT}
        let i = |v: &[i32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
        let m = |name, d: &[u8]| crate::archive::Member { name, data: d.to_vec().leak() };
        let ds = Object { members: vec![m("tstm_count", &i(&[4])), m("tst_type", &i(&[2, 1, 3, 4])), m("tst_value", &i(&[0x22, 7, 0, 1])), m("tstm_count", &i(&[3])), m("tst_type", &i(&[2, 1, 3])), m("tst_value", &i(&[2, 4, 1]))] };
        assert_eq!((tss(&ds, 0, D3DTSS_COLORARG1), tss(&ds, 0, D3DTSS_COLOROP)), (Some(0x22), Some(D3DTOP_ADD)));
        assert_eq!((tss(&ds, 1, D3DTSS_COLORARG1), tss(&ds, 1, D3DTSS_COLOROP)), (Some(2), Some(4)));
        assert_eq!(tss(&ds, 2, D3DTSS_COLOROP), None);
    }

    #[test]
    fn render_states_select_blend_and_cull() {
        let blend = |st: &[(i32, i32)]| material_mesh([1.0; 3], 1.0, st);
        assert_eq!(blend(&[(D3DRS_ALPHABLENDENABLE, 1)]).blend, Blend::AlphaBlend);
        assert_eq!(blend(&[(D3DRS_ALPHABLENDENABLE, 0)]).blend, Blend::Opaque);
        assert_eq!(blend(&[(D3DRS_ALPHATESTENABLE, 1)]).blend, Blend::AlphaTest);
        let both = blend(&[(D3DRS_ALPHATESTENABLE, 1), (D3DRS_ALPHABLENDENABLE, 1),
            (D3DRS_ALPHAREF, 30), (D3DRS_ALPHAFUNC, 5), (D3DRS_ZWRITEENABLE, 0)]);
        assert_eq!((both.blend, both.alpha_test, both.alpha_func, both.depth_write),
            (Blend::AlphaBlend, Some(30), 5, Some(false)));
        assert_eq!(blend(&[(D3DRS_ALPHABLENDENABLE, 1), (D3DRS_SRCBLEND, 2), (D3DRS_DESTBLEND, 2)]).blend, Blend::AdditiveOne);
        assert_eq!(blend(&[(D3DRS_ALPHABLENDENABLE, 1), (D3DRS_SRCBLEND, 5), (D3DRS_DESTBLEND, 2)]).blend, Blend::Additive);
        assert!(blend(&[(D3DRS_CULLMODE, D3DCULL_NONE)]).two_sided);
        assert!(!blend(&[(D3DRS_CULLMODE, 3)]).two_sided);
        let states = blend(&[(D3DRS_ZFUNC, 8), (D3DRS_FOGENABLE, 0), (D3DRS_LIGHTING, 0)]);
        assert_eq!(states.depth_func, Some(8));
        assert!(!states.fog_enabled && !states.lighting_enabled);
        assert_eq!(blend(&[(D3DRS_ALPHATESTENABLE, 1)]).alpha_test, Some(0));
    }
}
