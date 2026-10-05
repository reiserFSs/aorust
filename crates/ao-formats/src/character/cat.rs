//! `CATMesh` stream (rdb 1010002 / 1010027): skeleton + skinned geometry, see `docs/formats.md` § characters.
//!
//! Reader: `CATMesh_t::CATMesh_t(DataIO_t*, vector<string> const&)` @ randy31.dll 10052ff8.

use super::Rd;
use anyhow::{bail, ensure, Context, Result};

/// Texture-table entry that precedes the stream (one per material, same order and name).
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub name: String,
    /// rdb 1010004 diffuse texture id (0 = none).
    pub texture: u32,
    /// rdb 1010004 environment/second texture id (0 = none); only meaningful with `Material::flags & 2`.
    pub env_texture: u32,
    /// 1 when the texture's alpha channel is *not* transparency (always together with `flags & 8`).
    pub alpha_aux: u32,
}

/// `RMaterial_t` parameters (ctor @10041043).
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    pub name: String,
    /// bit 1: environment-map name follows; bit 3: alpha channel is not transparency
    /// (`RMaterial_t::IsAlphaUsedAsTransparency` == `!(flags >> 3 & 1)`); bits 0 and 2 are unused by the ctor.
    pub flags: u32,
    pub texture_name: String,
    pub env_name: Option<String>,
    pub diffuse: [f32; 3],
    pub specular: [f32; 3],
    pub ambient: [f32; 3],
    pub emissive: [f32; 3],
    pub shininess: f32,
    pub shininess_strength: f32,
    pub opacity: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Bone {
    pub name: String,
    /// Scales the translations of this bone's descendants (`FUN_100540a5`): 1.0 in all observed data.
    pub scale: f32,
    pub children: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkinVertex {
    /// Position in the local frame of `bones[0]` / `bones[1]`.
    pub local: [[f32; 3]; 2],
    /// Position in the bind pose (model space, D3D left-handed).
    pub bind: [f32; 3],
    /// Normal in the local frame of `bones[0]`.
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub bones: [u32; 2],
    /// Weight of `bones[0]`; `bones[1]` gets `1 - weight`.
    pub weight: f32,
}

#[derive(Clone, Debug)]
pub struct SubMesh {
    /// Index into `CatMesh::groups`' flattening order (selection group).
    pub group: usize,
    pub material: u32,
    pub vertices: Vec<SkinVertex>,
    /// Triangle list (D3D clockwise-front).
    pub indices: Vec<u16>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ColSphere {
    pub center: [f32; 3],
    pub radius: f32,
    pub bone: u32,
}

/// Named attachment point (`Attractor01_head`, `…_lefthand`: weapon/effect mounts).
#[derive(Clone, Debug, PartialEq)]
pub struct Attractor {
    pub name: String,
    pub pos: [f32; 3],
    pub rot: [f32; 4],
    pub scale: f32,
    pub bone: u32,
}

#[derive(Clone, Debug)]
pub struct CatMesh {
    /// The 32-byte name field: NUL-terminated root-bone name (bytes after the NUL are garbage/`0xCD`).
    pub root: String,
    pub parts: Vec<Part>,
    /// Skeleton hash; an animation fits this mesh iff `CatAnim::signature` is equal (`CATRender_t::SetAnim` @10053fe2).
    pub signature: u32,
    pub materials: Vec<Material>,
    /// Header-level 4-float records (`CATMesh_t+0x50`); empty in all observed data.
    pub spheres: Vec<[f32; 4]>,
    pub bones: Vec<Bone>,
    pub submeshes: Vec<SubMesh>,
    pub col_spheres: Vec<ColSphere>,
    pub attractors: Vec<Attractor>,
}

const PART_STRIDE_MAGIC: u32 = 1009;
const MAX_ITEMS: usize = 1 << 22;

fn count(r: &mut Rd, what: &str) -> Result<usize> {
    let n = r.u32()? as usize;
    ensure!(n <= MAX_ITEMS && n <= r.remaining(), "implausible {what} count {n}");
    Ok(n)
}

impl CatMesh {
    pub fn parse(d: &[u8]) -> Result<Self> {
        let mut r = Rd::new(d);
        let root = r.name32()?;
        // `(parts + 1) * 1009`: obfuscated count (all 778 records are a multiple).
        let k = r.u32()?;
        ensure!(k >= PART_STRIDE_MAGIC && k % PART_STRIDE_MAGIC == 0, "bad part table marker {k:#x}");
        let n = (k / PART_STRIDE_MAGIC - 1) as usize;
        ensure!(n * 44 <= r.remaining(), "part table overruns record");
        let mut parts = Vec::with_capacity(n);
        for _ in 0..n {
            let name = r.name32()?;
            parts.push(Part { name, texture: r.u32()?, env_texture: r.u32()?, alpha_aux: r.u32()? });
        }
        r.u32()?; // unidentified word between the table and the stream (varies per record, ignored by the reader)
        ensure!(r.u32()? == 4, "file is not a CATMesh");
        let version = r.u32()?;
        if version != 0x104 {
            bail!("unsupported CATMesh version {version:#x} (0x67 has no bind positions; not present in the data)");
        }
        let signature = r.u32()?;
        r.f32s::<2>()?; // CATMesh_t+0x2c / +0x30, unused
        let nm = count(&mut r, "material")?;
        let mut materials = Vec::with_capacity(nm);
        for _ in 0..nm {
            let name = r.string()?;
            let flags = r.u32()?;
            let texture_name = r.string()?;
            let env_name = if flags & 2 != 0 { Some(r.string()?) } else { None };
            let f = r.f32s::<15>()?;
            let rgb = |i: usize| [f[i], f[i + 1], f[i + 2]];
            materials.push(Material {
                name,
                flags,
                texture_name,
                env_name,
                diffuse: rgb(0),
                specular: rgb(3),
                ambient: rgb(6),
                emissive: rgb(9),
                shininess: f[12],
                shininess_strength: f[13],
                opacity: f[14],
            });
        }
        let ns = count(&mut r, "sphere")?;
        let spheres = (0..ns).map(|_| r.f32s::<4>()).collect::<Result<Vec<_>>>()?;
        let nb = count(&mut r, "bone")?;
        let mut bones = Vec::with_capacity(nb);
        for _ in 0..nb {
            let name = r.string()?;
            let scale = r.f32()?;
            let nc = count(&mut r, "bone child")?;
            let children = (0..nc).map(|_| r.u32()).collect::<Result<Vec<_>>>()?;
            ensure!(children.iter().all(|&c| (c as usize) < nb), "bone child out of range");
            bones.push(Bone { name, scale, children });
        }
        let ng = count(&mut r, "group")?;
        let (mut submeshes, mut col_spheres, mut attractors) = (Vec::new(), Vec::new(), Vec::new());
        for group in 0..ng {
            r.string()?; // group name ("-noselgroup-")
            let nsub = count(&mut r, "submesh")?;
            for _ in 0..nsub {
                let material = r.u32()?;
                ensure!((material as usize) < materials.len(), "invalid material index");
                let nv = count(&mut r, "vertex")?;
                let mut vertices = Vec::with_capacity(nv);
                for _ in 0..nv {
                    let [a, b, bind, normal] = [r.f32s::<3>()?, r.f32s::<3>()?, r.f32s::<3>()?, r.f32s::<3>()?];
                    let uv = r.f32s::<2>()?;
                    let bones_ = [r.u32()?, r.u32()?];
                    let weight = r.f32()?;
                    ensure!(bones_.iter().all(|&b| (b as usize) < nb), "vertex bone index out of range");
                    vertices.push(SkinVertex { local: [a, b], bind, normal, uv, bones: bones_, weight });
                }
                let ni = count(&mut r, "index")?;
                ensure!(ni % 3 == 0, "index count {ni} is not a multiple of 3");
                let indices = r.bytes(ni * 2)?.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>();
                ensure!(indices.iter().all(|&i| (i as usize) < nv), "index out of range");
                submeshes.push(SubMesh { group, material, vertices, indices });
            }
            for _ in 0..count(&mut r, "collision sphere")? {
                let [x, y, z, radius] = r.f32s::<4>()?;
                col_spheres.push(ColSphere { center: [x, y, z], radius, bone: r.u32()? });
            }
            for _ in 0..count(&mut r, "attractor")? {
                let name = r.string()?;
                let f = r.f32s::<8>()?;
                attractors.push(Attractor { name, pos: [f[0], f[1], f[2]], rot: [f[3], f[4], f[5], f[6]], scale: f[7], bone: r.u32()? });
            }
        }
        // What follows is either 12 zero bytes or a progressive-mesh table (`vertex_reorder`, `indices`,
        // `neighbours`, `splits`, `catindices`) used for LOD only; the full-detail mesh above is complete.
        let me = Self { root, parts, signature, materials, spheres, bones, submeshes, col_spheres, attractors };
        me.check().context("inconsistent CATMesh")?;
        Ok(me)
    }

    fn check(&self) -> Result<()> {
        ensure!(self.parts.len() == self.materials.len(), "{} texture parts for {} materials", self.parts.len(), self.materials.len());
        let mut state = vec![0u8; self.bones.len()]; // 0 new, 1 on stack, 2 done: reject cycles
        fn dfs(b: usize, bones: &[Bone], state: &mut [u8]) -> bool {
            match state[b] {
                1 => return false,
                2 => return true,
                _ => {}
            }
            state[b] = 1;
            let ok = bones[b].children.iter().all(|&c| dfs(c as usize, bones, state));
            state[b] = 2;
            ok
        }
        ensure!((0..self.bones.len()).all(|b| dfs(b, &self.bones, &mut state)), "bone hierarchy has a cycle");
        Ok(())
    }

    /// Parent bone of each bone (`None` for roots).
    pub fn parents(&self) -> Vec<Option<usize>> {
        let mut p = vec![None; self.bones.len()];
        for (i, b) in self.bones.iter().enumerate() {
            for &c in &b.children {
                p[c as usize] = Some(i);
            }
        }
        p
    }

    /// Bone indices ordered so that every parent precedes its children.
    pub fn bone_order(&self) -> Vec<usize> {
        let parents = self.parents();
        let mut order = Vec::with_capacity(self.bones.len());
        let mut stack: Vec<usize> = (0..self.bones.len()).rev().filter(|&b| parents[b].is_none()).collect();
        while let Some(b) = stack.pop() {
            order.push(b);
            stack.extend(self.bones[b].children.iter().rev().map(|&c| c as usize));
        }
        order
    }
}
