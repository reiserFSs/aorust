//! Poseable characters for the live world (players, NPCs, corpses): the textured model is built once ([`ActorRig::model`], an
//! `ao_scene` model for `Renderer::add_actor_model`), then every pose is skinned on the CPU from an animation clip
//! ([`ActorRig::pose`]): new body vertices plus the rigid mount transforms (head, weapons on attractor points).
//!
//! Mesh layout of [`ActorRig::model`]: `meshes[0]` = skinned body (a head part stub is dropped when a head mesh is mounted),
//! then the head (if any), then one mesh per attachment in the order given.

use super::*;
use crate::character::player::part_textures;
use ao_scene::Vertex;
use std::sync::Arc;

/// Shared caches of the loaders: the name table, per-model clip tables and parsed clips.
pub struct ActorAssets {
    pub names: NameTable,
    clips: HashMap<u32, Arc<Vec<(String, u32)>>>,
    anims: HashMap<u32, Arc<CatAnim>>,
}

impl ActorAssets {
    pub fn new(store: &RecordStore) -> Result<Self> {
        Ok(Self { names: NameTable::load(store)?, clips: HashMap::new(), anims: HashMap::new() })
    }

    /// Clip name -> clip id of a model (`walk`, `idle-stand`, `social-bow`, ...), cached ([`model_clips`]).
    pub fn clips(&mut self, store: &RecordStore, model: u32) -> Result<Arc<Vec<(String, u32)>>> {
        if let Some(c) = self.clips.get(&model) {
            return Ok(c.clone());
        }
        let c = Arc::new(model_clips(store, model)?);
        self.clips.insert(model, c.clone());
        Ok(c)
    }

    /// The parsed clip `id`, cached.
    pub fn anim(&mut self, store: &RecordStore, id: u32) -> Result<Arc<CatAnim>> {
        if let Some(a) = self.anims.get(&id) {
            return Ok(a.clone());
        }
        let a = Arc::new(load_anim(store, id)?);
        self.anims.insert(id, a.clone());
        Ok(a)
    }

    /// The clip of `role` on `model`, `None` if the model's set lacks it.
    pub fn role(&mut self, store: &RecordStore, model: u32, role: &Role) -> Result<Option<Arc<CatAnim>>> {
        let name = role.clone().clip_name();
        let id = self.clips(store, model)?.iter().find(|c| c.0 == name).map(|c| c.1);
        id.map(|id| self.anim(store, id)).transpose()
    }
}

/// Part textures of an NPC on `cat`: the wire `textures[]` list replaces the part textures (`SetCATTextures`, [`texture_overrides`]),
/// worn `cloth[]` `(part, rdb 1010004 texture)` is composited over the part's texture with the green key cut out (the client's
/// cloth-over-skin rule, [`overlay_on_skin`]; the consumer of the cloth table of morphed NPCs was not found, docs/zone/npc.md §6).
pub fn npc_part_textures(store: &RecordStore, cat: &CatMesh, list: &[TextureOverride], cloth: &[(ClothPart, u32)]) -> PartTextures {
    let load = |id: u32| {
        let key = TextureKey { rdb_type: TEXTURE_TYPE, id };
        load_texture(store, key).ok().flatten().map(|t| (key, t))
    };
    let mut out = PartTextures::new();
    for (i, o) in texture_overrides(cat, list) {
        if let Some(t) = (o.texture != 0).then(|| load(o.texture)).flatten() {
            out.insert(cat.parts[i].name.clone(), t);
        }
    }
    for &(part, id) in cloth {
        let Some(p) = cat.parts.iter().find(|p| p.name == part.name()) else { continue };
        let Some(over) = load(id) else { continue };
        let under = out.get(&p.name).cloned().or_else(|| (p.texture != 0).then(|| load(p.texture)).flatten());
        let entry = match under {
            Some((k, base)) => (TextureKey { rdb_type: 0x4000_0000 | k.id, id }, overlay_on_skin(&base, &over.1)),
            None => over,
        };
        out.insert(p.name.clone(), entry);
    }
    out
}

/// The env-texture / alpha-mode part of the wire `textures[]` list (`SetCATTexture` layers 3 and 1, [`texture_overrides`]) per part
/// name; parts the list does not touch keep the part table's env texture and the constructor's alpha mode ([`PartLayer`]).
pub fn npc_part_layers(cat: &CatMesh, list: &[TextureOverride]) -> PartLayers {
    texture_overrides(cat, list)
        .into_iter()
        .map(|(i, o)| (cat.parts[i].name.clone(), PartLayer { env_texture: o.env_texture, alpha_mode: (o.texture != 0).then_some(o.alpha_mode) }))
        .collect()
}

/// What a player looks like on the wire (`SimpleCharFullUpdate` packed word, head, cloth).
#[derive(Clone, Debug)]
pub struct PlayerLook {
    pub breed: Breed,
    pub gender: Gender,
    pub skin: Skin,
    /// 0 thin, 1 normal, 2 fat (`Fatness`, stat 0x2F).
    pub build: u8,
    /// rdb 1010001 head mesh id (`HeadMesh`, stat 0x40).
    pub head: Option<u32>,
    pub equipment: Equipment,
}

struct Mount {
    /// Index into `CatMesh::attractors`.
    attractor: usize,
    /// Index into `ActorRig::model().meshes`.
    mesh: usize,
}

pub struct ActorRig {
    cat: CatMesh,
/// The attractor meshes `(place, rdb 1010001 mesh)` a character carries after the client applied its full update
/// (`FUN_10077e13` [GC 0x10077e13], only when message flag bit 2 `SET_DYNEL_800` is clear):
/// `CharacterMesh::AddAttractorMesh(0, HeadMesh)`, then **`CharacterMesh::ClearAttractors`** [DS 0x10071dd0] (deletes every node of
/// the attractor list, the head just added included), then `CharacterMesh::AddAttractors(wire list)` (per entry
/// `AddAttractorMesh` [DS 0x10071cce]). So the wire `HeadMesh` stat never reaches the model by itself: the head is the place-0
/// entry of the wire list (the later runtime head change `FUN_10059376` removes/adds place 0 the same way). The list is ordered
/// by place; a new entry is inserted before the first node whose place is `>=` its own (equal places: the later one first).
/// `head` is `HeadMesh` (non-zero only), `wire` the message's attractor list.
pub fn attractor_list(head: Option<u32>, wire: &[(u8, u32)]) -> Vec<(u8, u32)> {
    let mut list: Vec<(u8, u32)> = vec![];
    let add = |list: &mut Vec<(u8, u32)>, e: (u8, u32)| {
        let at = list.iter().position(|n| n.0 >= e.0).unwrap_or(list.len());
        list.insert(at, e);
    };
    if let Some(h) = head {
        add(&mut list, (0, h));
    }
    list.clear(); // ClearAttractors
    for &e in wire {
        add(&mut list, e);
    }
    list
}

    parents: Vec<Option<usize>>,
    scale: Vec<f32>,
    order: Vec<usize>,
    /// Bind-pose world frame of every bone that has one (`None` = no skin vertex is fully weighted to it).
    bind: Vec<Option<Xf>>,
    /// `cat.submeshes` indices in the order their vertices appear in `model.meshes[0]`.
    used: Vec<usize>,
    model: Scene,
    mounts: Vec<Mount>,
    /// The attractor of the head mesh.
    head: Option<usize>,
    /// Model id (rdb 1010002) this rig was built from.
    pub model_id: u32,
}

impl ActorRig {
    /// Body model `model_id` (rdb 1010002), optional head mesh (rdb 1010001), per-material texture `overrides` and env/alpha `layers`
    /// ([`npc_part_layers`]) and attachment
    /// meshes `(attractor place, rdb 1010001 mesh)` (`Attractor01_head` = place 0, `02_righthand` = 1, ...; unknown places are skipped).
    pub fn new(store: &RecordStore, model_id: u32, head: Option<u32>, overrides: &PartTextures, layers: &PartLayers, attachments: &[(u8, u32)]) -> Result<Self> {
        let cat = load_cat_mesh(store, CHAR_MESH_TYPE, model_id)?;
        // creature models have no head attractor: a head mesh is then not mounted (and the body keeps its own head part)
        let head = head.filter(|_| cat.attractors.iter().any(|a| a.name.ends_with("_head")));
        let bind = bind_frames(&cat);
        let skin = skin_bind(&cat, &bind);
        let (mut model, ..) = assemble(store, &cat, &skin, overrides, layers, head.is_some());
        let used = (0..cat.submeshes.len()).filter(|&i| !(head.is_some() && cat.parts[cat.submeshes[i].material as usize].name == "head")).collect();
        let mut mounts = vec![];
        let mut head_att = None;
        if let Some(h) = head {
            let att = cat.attractors.iter().position(|a| a.name.ends_with("_head")).context("model has no head attractor")?;
            if let Some(mesh) = crate::mesh::decode_mesh_into(store, h, &mut model)? {
                mounts.push(Mount { attractor: att, mesh });
                head_att = Some(att);
            }
        }
        for &(place, id) in attachments {
            let prefix = format!("Attractor{:02}", place as u32 + 1);
            let Some(att) = cat.attractors.iter().position(|a| a.name.starts_with(&prefix)) else { continue };
            if let Some(mesh) = crate::mesh::decode_mesh_into(store, id, &mut model)? {
                mounts.push(Mount { attractor: att, mesh });
            }
        }
        model.instances.clear();
        let parents = cat.parents();
        let scale = translation_scales(&cat);
        let order = cat.bone_order();
        Ok(Self { cat, parents, scale, order, bind, used, model, mounts, head: head_att, model_id })
    }

    /// A player: body of breed/sex/build, naked skin + worn cloth composite, head mesh, attachments.
    pub fn player(store: &RecordStore, assets: &ActorAssets, look: &PlayerLook, attachments: &[(u8, u32)]) -> Result<Self> {
        let model = player_model_build(store, look.breed, look.gender, look.build)?;
        let p = Player { breed: look.breed, gender: look.gender, skin: look.skin, head: None, equipment: look.equipment };
        let overrides = part_textures(&assets.names, store, model, &p)?;
        Self::new(store, model, look.head, &overrides, &PartLayers::new(), attachments)
    }

    /// The textured model for `Renderer::add_actor_model`.
    pub fn model(&self) -> &Scene {
        &self.model
    }

    pub fn cat(&self) -> &CatMesh {
        &self.cat
    }

    /// Body height in metres (bind pose), for name-tag placement.
    pub fn height(&self) -> f32 {
        self.model.meshes[0].vertices.iter().map(|v| v.pos[1]).fold(0.0, f32::max) + if self.head.is_some() { 0.25 } else { 0.0 }
    }

    /// Height (metres above the feet, bind pose, unscaled) of the name tag / indicator anchor: `Attractor01_head` + 0.5 m
    /// (`VisualCATMesh_t::GetIndicatorPosition`, docs/zone/motion.md §6); models without a head attractor: body height + 0.3.
    pub fn indicator_height(&self) -> f32 {
        let anchor = self.cat.attractors.iter().position(|a| a.name.ends_with("_head")).and_then(|i| {
            let att = &self.cat.attractors[i];
            let bone = self.bind[att.bone as usize].or_else(|| self.nearest_frame(&self.bind, att.bone as usize))?;
            Some(bone.mul(&Xf::from_qt(att.rot, att.pos)).t[1] + 0.5)
        });
        anchor.unwrap_or_else(|| self.height() + 0.3)
    }

    /// World frame of every bone in the pose (`FUN_100540a5`).
    fn world(&self, pose: Option<(&CatAnim, f32)>) -> Vec<Xf> {
        let mut world = vec![Xf::ID; self.cat.bones.len()];
        for &b in &self.order {
            let (q, t) = pose.and_then(|(a, ms)| a.sample(b, ms)).unwrap_or(([0.0, 0.0, 0.0, 1.0], [0.0; 3]));
            let local = Xf::from_qt(q, t.map(|c| c * self.scale[b]));
            world[b] = self.parents[b].map_or(local, |p| world[p].mul(&local));
        }
        world
    }

    /// Body vertices (same layout as `model().meshes[0]`) and the per-mesh mount transforms (column-major, relative to the actor)
    /// for `clip` at `ms` milliseconds (looped); `None` = bind pose. `parts[0]` is identity.
    pub fn pose(&self, clip: Option<(&CatAnim, f32)>) -> (Vec<Vertex>, Vec<[[f32; 4]; 4]>) {
        let clip = clip.filter(|(a, _)| a.signature == self.cat.signature).map(|(a, ms)| (a, if a.duration > 0.0 { ms.rem_euclid(a.duration) } else { 0.0 }));
        let mut verts = self.model.meshes[0].vertices.clone();
        let frames: Vec<Option<Xf>> = match clip {
            Some(_) => self.world(clip).into_iter().map(Some).collect(),
            None => self.bind.clone(),
        };
        if clip.is_some() {
            let world: Vec<Xf> = frames.iter().map(|f| f.unwrap()).collect();
            let mut out = verts.iter_mut();
            for &si in &self.used {
                for v in &self.cat.submeshes[si].vertices {
                    let m0 = &world[v.bones[0] as usize];
                    let p0 = m0.apply(v.local[0]);
                    let p = if v.weight >= SINGLE_BONE_WEIGHT {
                        p0
                    } else {
                        let p1 = world[v.bones[1] as usize].apply(v.local[1]);
                        std::array::from_fn(|i| v.weight * p0[i] + (1.0 - v.weight) * p1[i])
                    };
                    let n = unit(m0.rot(v.normal));
                    if let Some(o) = out.next() {
                        o.pos = [p[0], p[1], -p[2]];
                        o.normal = [n[0], n[1], -n[2]];
                    }
                }
            }
        }
        let mut parts = vec![IDENTITY; self.model.meshes.len()];
        for m in &self.mounts {
            let att = &self.cat.attractors[m.attractor];
            let Some(bone) = frames[att.bone as usize].or_else(|| self.nearest_frame(&frames, att.bone as usize)) else { continue };
            let w = bone.mul(&Xf::from_qt(att.rot, att.pos));
            // mount meshes are already mirrored (Z negated): conjugate the transform by the mirror
            let flip = |i: usize, j: usize| if (i == 2) != (j == 2) { -1.0 } else { 1.0 };
            let mut t = IDENTITY;
            for (c, col) in t.iter_mut().enumerate().take(3) {
                for (r, x) in col.iter_mut().enumerate().take(3) {
                    *x = w.r[r][c] * flip(r, c);
                }
            }
            t[3] = [w.t[0], w.t[1], -w.t[2], 1.0];
            parts[m.mesh] = t;
        }
        (verts, parts)
    }

    /// Frame of the closest ancestor that has one (a bone without skin vertices in the bind pose).
    fn nearest_frame(&self, frames: &[Option<Xf>], mut b: usize) -> Option<Xf> {
        loop {
            b = self.parents[b]?;
            if let Some(f) = frames[b] {
                return Some(f);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Option<RecordStore> {
        let dir = std::path::PathBuf::from(std::env::var_os("HOME")?).join("Games/ProjectRubiKa/client");
        RecordStore::open(&dir).ok()
    }

    /// A solitus female with head and a weapon on the right hand: the body has the model's vertices, the head and the weapon
    /// follow their attractor, a run clip moves the body and the head mount.
    #[test]
    fn player_rig_poses_body_and_mounts() {
        let Some(store) = store() else { return };
        let mut assets = ActorAssets::new(&store).unwrap();
        let look = PlayerLook { breed: Breed::Solitus, gender: Gender::Female, skin: Skin::Caucasian, build: 1, head: Some(40629), equipment: Equipment::default() };
        let rig = ActorRig::player(&store, &assets, &look, &[(1, 7796)]).unwrap();
        assert_eq!(rig.model().meshes.len(), 3, "body, head, weapon");
        let run = assets.role(&store, rig.model_id, &Role::Run).unwrap().expect("run clip");
        let (bind, bind_parts) = rig.pose(None);
        let (v0, p0) = rig.pose(Some((&run, 0.0)));
        let (v1, p1) = rig.pose(Some((&run, run.duration / 2.0)));
        assert_eq!(bind.len(), v0.len());
        assert_ne!(v0, v1, "the clip moves the body");
        assert_ne!(p0[1], p1[1], "the head follows its bone");
        assert_ne!(p0[2], p1[2], "the weapon follows the hand");
        assert_eq!(p0[0], IDENTITY, "the body has no mount transform");
        let h = rig.indicator_height();
        assert!((1.5..2.6).contains(&h), "name tag anchor {h}");
        assert!(bind_parts[1][3][1] > 1.2, "bind-pose head at the shoulders: {}", bind_parts[1][3][1]);
    }

    /// The Surf Lizard (record 22794 -> model 22773): `textures[]` replaces the part texture, a creature without `HeadMesh` mounts nothing.
    #[test]
    fn npc_rig_applies_texture_overrides() {
        let Some(store) = store() else { return };
        let cat = load_cat_mesh(&store, CHAR_MESH_TYPE, 22773).unwrap();
        let list = [TextureOverride { material: "lizard_green", texture: 22768, env_texture: 0, alpha_mode: 0 }];
        let o = npc_part_textures(&store, &cat, &list, &[]);
        assert_eq!(o["lizard_green"].0, TextureKey { rdb_type: TEXTURE_TYPE, id: 22768 });
        let rig = ActorRig::new(&store, 22773, None, &o, &PartLayers::new(), &[]).unwrap();
        assert_eq!(rig.model().meshes.len(), 1, "body only");
        assert!(rig.model().textures.contains_key(&TextureKey { rdb_type: TEXTURE_TYPE, id: 22768 }));
    }

    /// `ClearAttractors` drops the head `AddAttractorMesh(0, HeadMesh)` just added: only the wire list survives, ordered by place.
    #[test]
    fn clear_attractors_drops_the_head_mesh() {
        assert_eq!(attractor_list(Some(40629), &[]), vec![]);
        assert_eq!(attractor_list(Some(40629), &[(5, 26163), (0, 40103)]), vec![(0, 40103), (5, 26163)]);
        // equal places: the later insert goes before the earlier one
        assert_eq!(attractor_list(None, &[(1, 7), (1, 8), (0, 3)]), vec![(0, 3), (1, 8), (1, 7)]);
    }

    /// Wire env texture / alpha mode reach the part: only a part whose layer-1 texture was replaced takes the wire alpha mode.
    #[test]
    fn wire_layers_per_part() {
        let part = |n: &str| Part { name: n.into(), texture: 1, env_texture: 2, alpha_aux: 0 };
        let cat = CatMesh { root: String::new(), parts: vec![part("arms"), part("body")], signature: 0, materials: vec![], spheres: vec![], bones: vec![], submeshes: vec![], col_spheres: vec![], attractors: vec![] };
        let t = |material, texture, env_texture, alpha_mode| TextureOverride { material, texture, env_texture, alpha_mode };
        let l = npc_part_layers(&cat, &[t("body", 7, 0, 5), t("arms", 0, 9, 0), t("none", 1, 1, 1)]);
        assert_eq!(l["body"], PartLayer { env_texture: 0, alpha_mode: Some(5) });
        assert_eq!(l["arms"], PartLayer { env_texture: 9, alpha_mode: None }, "env only: alpha mode stays the constructor's");
        assert_eq!(l.len(), 2);
    }

    /// Model 42370 has a part with an environment texture (`env_sleek.png`): the rig's submesh carries it, the wire layer replaces it.
    #[test]
    fn part_env_texture_reaches_the_submesh() {
        let Some(store) = store() else { return };
        let cat = load_cat_mesh(&store, CHAR_MESH_TYPE, 42370).unwrap();
        let Some(p) = cat.parts.iter().find(|p| p.env_texture != 0) else { return };
        let envs = |layers: &PartLayers| {
            let rig = ActorRig::new(&store, 42370, None, &PartTextures::new(), layers, &[]).unwrap();
            rig.model().meshes[0].submeshes.iter().filter_map(|s| s.env_texture.map(|k| k.id)).collect::<Vec<_>>()
        };
        assert!(envs(&PartLayers::new()).contains(&p.env_texture));
        let swapped = PartLayers::from([(p.name.clone(), PartLayer { env_texture: 22768, alpha_mode: None })]);
        assert!(envs(&swapped).contains(&22768));
    }
}
