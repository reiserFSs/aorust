//! Authored visual effect templates (`Setupf/gfxtweak.bin`).
//! GC `FUN_10106be2` reads the record count, then `CMSBlock` records.
use anyhow::{ensure, Context, Result};
use std::collections::HashMap;
use std::path::Path;

/// `FUN_100a761f` / `100a7803` / `100a7863` → `FUN_1009ad2c`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    pub group: u8,
    pub attractor: i32,
    pub effect: i32,
    pub note: i32,
    pub color: u32,
}

/// Each group holds four tuples; a later tuple with the same note replaces it.
pub fn bindings(spells: &[ao_net::n3::spells::Spell]) -> Vec<Binding> {
    let mut out: Vec<Binding> = Vec::new();
    for spell in spells {
        let group = match spell.function { 0xcf49 => 0, 0xcf53 => 1, 0xcf54 => 2, _ => continue };
        let effect = spell.stat(0x57);
        let binding = Binding { group, attractor: spell.stat(0x56), effect: if effect == 2710 { 62002 } else { effect }, note: spell.stat(0x49), color: spell.stat(0x59) as u32 };
        if let Some(old) = out.iter_mut().find(|b| b.group == group && b.note == binding.note) {
            *old = binding;
        } else if out.iter().filter(|b| b.group == group).count() < 4 {
            out.push(binding);
        }
    }
    out
}

impl Binding {
    /// Muzzle default note zero means attack 0xb; hit defaults match every note.
    pub fn fires(self, note: i32, hit: bool) -> bool {
        (self.group != 2 || hit) && (self.note == note || (self.note == 0 && (self.group != 0 || note == 0xb)))
    }
}

#[derive(Clone, Debug)]
pub struct Template {
    pub kind: i32,
    pub words: Vec<u32>,
}

#[derive(Default)]
pub struct Templates {
    pub by_id: HashMap<i32, Template>,
}

impl Templates {
    pub fn open(dir: &Path) -> Result<Self> {
        Self::parse(&std::fs::read(dir.join("Setupf/gfxtweak.bin"))?)
    }

    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let mut at = 0usize;
        let word = |at: &mut usize| -> Result<u32> {
            let end = at.checked_add(4).context("effect offset overflow")?;
            let b = bytes.get(*at..end).context("truncated effect table")?;
            *at = end;
            Ok(u32::from_le_bytes(b.try_into().unwrap()))
        };
        let count = word(&mut at)? as usize;
        ensure!(count <= bytes.len().saturating_sub(at) / 12, "effect count exceeds table");
        let mut by_id = HashMap::with_capacity(count);
        for _ in 0..count {
            let id = word(&mut at)? as i32;
            let kind = word(&mut at)? as i32;
            let n = word(&mut at)? as usize;
            ensure!(n <= bytes.len().saturating_sub(at) / 4, "effect payload exceeds table");
            let mut words = Vec::with_capacity(n);
            for _ in 0..n { words.push(word(&mut at)?); }
            ensure!(by_id.insert(id, Template { kind, words }).is_none(), "duplicate effect id {id}");
        }
        ensure!(at == bytes.len(), "trailing effect table bytes");
        Ok(Self { by_id })
    }
}

#[path = "effects_materials.rs"]
mod materials;

use ao_formats::character::{CrtRand, NameTable};
use ao_rdb::RecordStore;
use ao_scene::{ActorFrame, Blend, Mesh, Scene, Submesh, TextureKey, Vertex, IDENTITY};
use glam::{Mat4, Vec3};

const MODEL_BASE: u64 = 0xfac0_0000_0000_0000;
const ACTOR_BASE: u32 = 0xe000_0000;

impl Template {
    fn word(&self, i: usize) -> Result<u32> {
        self.words.get(i).copied().with_context(|| format!("short effect class {} at parameter {i}", self.kind))
    }
    fn float(&self, i: usize) -> Result<f32> {
        let f = f32::from_bits(self.word(i)?);
        ensure!(f.is_finite(), "nonfinite effect class {} parameter {i}", self.kind);
        Ok(f)
    }
}

#[derive(Clone, Copy)]
struct Particle {
    position: Vec3,
    velocity_p: Vec3,
    velocity_q: Vec3,
    life: f32,
}

struct Active {
    actor: u32,
    effect: i32,
    source: Mat4,
    target: Vec3,
    elapsed: f32,
    color: u32,
    particles: Vec<Particle>,
}

struct EffectModel {
    scene: Scene,
    uploaded: bool,
}

/// CPU effect simulation, using the existing dynamic actor upload/render pass.
/// No substitute texture or substitute effect is created when retail data is absent.
pub struct Renderer {
    templates: Templates,
    store: RecordStore,
    names: NameTable,
    models: HashMap<i32, EffectModel>,
    active: Vec<Active>,
    rng: CrtRand,
    next_actor: u32,
    generation: u64,
}

impl Renderer {
    pub fn open(dir: &Path) -> Result<Self> {
        let store = RecordStore::open(dir)?;
        let names = NameTable::load(&store)?;
        Ok(Self { templates: Templates::open(dir)?, store, names, models: HashMap::new(), active: vec![], rng: CrtRand::new(1), next_actor: ACTOR_BASE, generation: u64::MAX })
    }

    pub fn clear(&mut self) { self.active.clear(); }

    /// Default connector attractor (parameter 7), overridden by a nonzero item tuple.
    pub fn attractor(&self, effect: i32, explicit: i32) -> Option<i32> {
        if explicit != 0 { return Some(explicit); }
        self.templates.by_id.get(&effect)?.words.get(7).map(|&v| v as i32)
    }

    pub fn spawn(&mut self, binding: Binding, source: Mat4, target: Vec3) -> Result<()> {
        if binding.effect == 49999 { return Ok(()); }
        let template = self.templates.by_id.get(&binding.effect).with_context(|| format!("missing effect {}", binding.effect))?;
        ensure!(matches!(template.kind, 1005 | 1025), "unsupported authored weapon effect {} class {}", binding.effect, template.kind);
        let count = if template.kind == 1005 { template.word(31)? as usize } else { 1 };
        ensure!((1..=4096).contains(&count), "invalid effect particle count {count}");
        let material = template.word(9)? as usize;
        let &(name, columns, rows, first, last) = materials::MATERIALS.get(material).context("unknown effect material")?;
        if !self.models.contains_key(&binding.effect) {
            let key = TextureKey { rdb_type: 1010004, id: self.names.id(1010004, name).with_context(|| format!("missing effect texture {name}"))? };
            let texture = ao_formats::texture::load_texture(&self.store, key)?.with_context(|| format!("missing effect texture {}", key.id))?;
            let mut sub = Submesh::new((0..count as u32).flat_map(|i| [i*4, i*4+2, i*4+3, i*4, i*4+3, i*4+1]).collect(), Some(key));
            sub.blend = Blend::Additive;
            sub.two_sided = true;
            sub.emissive = [1.0; 3];
            let mut scene = Scene::default();
            scene.textures.insert(key, texture);
            scene.meshes.push(Mesh { vertices: vec![Vertex::default(); count*4], submeshes: vec![sub] });
            self.models.insert(binding.effect, EffectModel { scene, uploaded: false });
        }
        // GC 100dcd93: uniform azimuth, elevation and speed, then connector rotation.
        let mut particles = Vec::with_capacity(count);
        if template.kind == 1005 {
            let range = |a, b| -> Result<(f32, f32)> { Ok((template.float(a)?, template.float(b)?)) };
            let (az0, az1) = range(25, 26)?;
            let (el0, el1) = range(27, 28)?;
            let (v0, v1) = range(29, 30)?;
            let (l0, l1) = range(34, 35)?;
            ensure!(l0 > 0.0 && l1 > 0.0, "invalid effect sprite lifetime");
            for _ in 0..count {
                let mut random = || self.rng.rand() as f32 / 32767.0;
                let az = az0 + (az1-az0)*random();
                let el = el0 + (el1-el0)*random();
                let speed = v0 + (v1-v0)*random();
                let velocity = Vec3::new(az.cos()*el.cos(), az.sin()*el.cos(), el.sin())*speed;
                let life = l0 + (l1-l0)*random();
                let (p, q) = if template.word(0)? & 0x100 == 0 { (template.float(32)?, template.float(33)?) } else { (template.float(33)?, template.float(32)?) };
                particles.push(Particle { position: Vec3::ZERO, velocity_p: velocity*(p/life), velocity_q: velocity*(q/life), life });
            }
        }
        let _ = (columns, rows, first, last);
        let actor = self.next_actor;
        self.next_actor = self.next_actor.wrapping_add(1).max(ACTOR_BASE);
        self.active.push(Active { actor, effect: binding.effect, source, target, elapsed: 0.0, color: binding.color, particles });
        Ok(())
    }

    /// GC 10100104: the moving cord occupies `[speed*t, speed*t+length]`,
    /// clamped at the real hit location. Width is parameter 12.
    fn projectile(template: &Template, source: Vec3, target: Vec3, time: f32) -> Result<Option<(Vec3, Vec3, f32)>> {
        let delta = target-source;
        let distance = delta.length();
        let speed = template.float(10)?.min(distance*5.0);
        ensure!(speed > 0.0, "invalid projectile speed");
        let tail = speed*time;
        if tail > distance { return Ok(None); }
        let head = (tail+template.float(11)?).min(distance);
        let direction = delta / distance;
        Ok(Some((source+direction*tail, source+direction*head, template.float(12)?)))
    }

    pub fn frame(&mut self, dt: f32, host: &mut ao_render::Host) {
        if self.generation != host.scene_generation() {
            self.generation = host.scene_generation();
            for model in self.models.values_mut() { model.uploaded = false; }
        }
        for (id, model) in &mut self.models {
            if !model.uploaded {
                host.actor_models.push((MODEL_BASE | *id as u32 as u64, model.scene.clone()));
                model.uploaded = true;
            }
        }
        let right = host.camera.right();
        let up = host.camera.up();
        let camera = host.camera.pos;
        let forward = host.camera.forward();
        self.active.retain_mut(|effect| {
            let template = &self.templates.by_id[&effect.effect];
            effect.elapsed += dt.abs();
            let mut skin = Vec::with_capacity(self.models[&effect.effect].scene.meshes[0].vertices.len());
            if template.kind == 1025 {
                let Ok(Some((tail, head, width))) = Self::projectile(template, effect.source.w_axis.truncate(), effect.target, effect.elapsed) else { return false };
                let side = (head-tail).cross(camera-(head+tail)*0.5).normalize_or_zero()*width;
                let color = tint(template, effect.color, 0.0, 13);
                quad(&mut skin, [tail-side, tail+side, head-side, head+side], [[0.0,0.0], [0.0,1.0], [1.0,0.0], [1.0,1.0]], color);
            } else {
                let material = template.words[9] as usize;
                let (_, columns, rows, first, last) = materials::MATERIALS[material];
                let mut alive = false;
                for p in &effect.particles {
                    let t = (effect.elapsed/p.life).min(1.0);
                    let live = effect.elapsed < p.life;
                    alive |= live;
                    let radius = lerp(template, 12, 13, t);
                    let p_world = effect.source.transform_point3(p.position+p.velocity_p*effect.elapsed);
                    let q_world = effect.source.transform_point3(p.position+p.velocity_q*effect.elapsed);
                    let view = |p: Vec3| { let d=p-camera; glam::Vec2::new(d.dot(right),d.dot(up))/d.dot(forward) };
                    let d = (view(q_world)-view(p_world)).normalize_or_zero();
                    let (a,b) = if d == glam::Vec2::ZERO { (right*radius,up*radius) } else { ((right*d.x+up*d.y)*radius,(right*d.y-up*d.x)*radius) };
                    let frame = (first as f32+(last-first) as f32*t) as u32;
                    let (x,y) = (frame%columns, frame/rows);
                    let uv = [[x as f32/columns as f32,(y+1) as f32/rows as f32], [x as f32/columns as f32,y as f32/rows as f32], [(x+1) as f32/columns as f32,(y+1) as f32/rows as f32], [(x+1) as f32/columns as f32,y as f32/rows as f32]];
                    let mut color = tint(template, effect.color, t, 16);
                    if !live { color[3] = 0.0; }
                    quad(&mut skin, [p_world-a-b, p_world-a+b, q_world+a-b, q_world+a+b], uv, color);
                }
                if !alive { return false; }
            }
            host.actors.push(ActorFrame { id: effect.actor, model: MODEL_BASE | effect.effect as u32 as u64, transform: IDENTITY, parts: vec![], skin: Some(skin), always: false, alpha: 1.0 });
            true
        });
    }
}

fn lerp(template: &Template, a: usize, b: usize, t: f32) -> f32 {
    let a = f32::from_bits(template.words[a]);
    a+(f32::from_bits(template.words[b])-a)*t
}

fn tint(template: &Template, override_color: u32, t: f32, first: usize) -> [f32; 4] {
    let rgba = if override_color == 0 {
        if first == 16 { [lerp(template,17,21,t),lerp(template,18,22,t),lerp(template,19,23,t),lerp(template,16,20,t)] }
        else { [f32::from_bits(template.words[first+1]),f32::from_bits(template.words[first+2]),f32::from_bits(template.words[first+3]),f32::from_bits(template.words[first])] }
    } else {
        let [a,r,g,b] = override_color.to_be_bytes().map(|v| v as f32/255.0);
        [r,g,b,a*(1.0-t)]
    };
    [rgba[0].max(0.0).powf(2.2),rgba[1].max(0.0).powf(2.2),rgba[2].max(0.0).powf(2.2),rgba[3]]
}

fn quad(vertices: &mut Vec<Vertex>, corners: [Vec3; 4], uvs: [[f32; 2]; 4], color: [f32; 4]) {
    for (pos, uv) in corners.into_iter().zip(uvs) {
        vertices.push(Vertex { pos: pos.to_array(), normal: [0.0,0.0,1.0], uv, color });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_envelope_is_bounded_and_preserves_float_bits() {
        let words = [1u32, 1000, 1001, 2, 5, 0x3f800000];
        let bytes: Vec<_> = words.into_iter().flat_map(u32::to_le_bytes).collect();
        let table = Templates::parse(&bytes).unwrap();
        assert_eq!(table.by_id[&1000].kind, 1001);
        assert_eq!(table.by_id[&1000].words, [5, 0x3f800000]);
        for n in 0..bytes.len() { assert!(Templates::parse(&bytes[..n]).is_err()); }
        let mut bad = bytes.clone();
        bad[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Templates::parse(&bad).is_err());
        bad = bytes;
        bad.push(0);
        assert!(Templates::parse(&bad).is_err());
    }

    #[test]
    fn weapon_bindings_replace_notes_and_gate_only_impacts_on_hits() {
        use ao_net::n3::spells::Spell;
        let make = |function, effect, note| Spell { function, stats: [(0x56, 0), (0x57, effect), (0x49, note), (0x59, -22016)].into(), ..Default::default() };
        let b = bindings(&[make(0xcf49, 2000, 0x73), make(0xcf49, 2005, 0x73), make(0xcf53, 2750, 0), make(0xcf54, 2710, 0)]);
        assert_eq!(b.len(), 3);
        assert_eq!((b[0].effect, b[0].color), (2005, 0xffffaa00));
        assert!(b[0].fires(0x73, false));
        assert!(!b[0].fires(0xb, true));
        assert!(b[1].fires(0x73, false), "tracers also accompany misses");
        assert!(!b[2].fires(0x73, false));
        assert!(b[2].fires(0x73, true), "normal successful hits, not only critical hits");
        assert_eq!(b[2].effect, 62002);
    }

    #[test]
    fn actual_rifle_smoke_cord_and_impact_use_authored_art_and_parameters() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let dir = std::path::PathBuf::from(home).join("Games/ProjectRubiKa/client");
        if !dir.join("Setupf/gfxtweak.bin").exists() || !dir.join("cd_image/rdb.db").exists() { return; }
        let mut renderer = Renderer::open(&dir).unwrap();
        for (group, effect) in [(0,2005), (1,2750), (2,62002)] {
            renderer.spawn(Binding { group, attractor: 0, effect, note: 0x73, color: 0 }, Mat4::IDENTITY, Vec3::X*10.0).unwrap();
        }
        assert_eq!(renderer.active[0].particles.len(), 64);
        assert_eq!(renderer.active[2].particles.len(), 128);
        assert_eq!(renderer.templates.by_id[&2005].word(9).unwrap(), 31, "retail x_smoke.png");
        assert_eq!(renderer.templates.by_id[&2750].word(9).unwrap(), 15, "retail s_bullet.png");
        assert_eq!(renderer.templates.by_id[&62002].word(9).unwrap(), 15, "impact streaks share s_bullet.png");
        let (tail, head, width) = Renderer::projectile(&renderer.templates.by_id[&2750], Vec3::ZERO, Vec3::X*10.0, 0.1).unwrap().unwrap();
        assert_eq!(tail, Vec3::X*2.5);
        assert_eq!(head, Vec3::X*3.75);
        assert_eq!(width, 0.0625);
        assert!(Renderer::projectile(&renderer.templates.by_id[&2750], Vec3::ZERO, Vec3::X*10.0, 0.5).unwrap().is_none());
    }

    #[test]
    fn retail_table_has_authored_weapon_and_font_templates() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let dir = std::path::PathBuf::from(home).join("Games/ProjectRubiKa/client");
        if !dir.join("Setupf/gfxtweak.bin").exists() { return; }
        let table = Templates::open(&dir).unwrap();
        assert_eq!(table.by_id.len(), 2687);
        assert_eq!(table.by_id[&1000].kind, 1001);
        assert_eq!(table.by_id[&0x2f5a].kind, 0x7de);
    }
}
