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
#[path = "effects_beams.rs"]
mod beams;
#[path = "effects_sprites.rs"]
mod sprites;
#[path = "effects_control.rs"]
mod control;
#[path = "effects_composition.rs"]
mod composition;
#[path = "effects_meshes.rs"]
mod meshes;
#[path = "effects_tracer_sprites.rs"]
mod tracer_meshes;
#[path = "effects_aux.rs"]
mod aux;
pub use aux::AuxSound;
#[path = "effects_particles.rs"]
mod particles;
#[path = "effects_particles2.rs"]
mod particles2;
#[path = "effects_replicated.rs"]
mod replicated;
#[path = "effects_crystal.rs"]
mod crystal;
#[path = "effects_buffs.rs"]
mod buffs;
#[path = "effects_buff200x.rs"]
mod buff200x;
#[path = "effects_buffelectra.rs"]
mod buffelectra;
#[path = "effects_buffstars.rs"]
mod buffstars;
#[path = "effects_buff300x.rs"]
mod buff300x;
#[path = "effects_buff_shield.rs"]
mod buff_shield;
#[path = "effects_buff303x.rs"]
mod buff303x;
#[path = "effects_highlight.rs"]
mod highlight;
#[cfg(test)]
#[path = "effects_survey.rs"]
mod survey;

use ao_formats::character::{CrtRand, NameTable};
use ao_formats::weather::R250;
use ao_rdb::RecordStore;
use ao_scene::{ActorFrame, Blend, Mesh, Scene, Submesh, TextureKey, Vertex, IDENTITY};
use glam::{Mat4, Vec3};

const MODEL_BASE: u64 = 0xfac0_0000_0000_0000;
const ACTOR_BASE: u32 = 0xe000_0000;
const MESH_MODEL_BASE: u64 = 0xfac1_0000_0000_0000;

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
    born: f32,
    radius_scale: f32,
}

struct Active {
    actor: u32,
    effect: i32,
    raw_source: Mat4,
    source: Mat4,
    target: Vec3,
    elapsed: f32,
    color: u32,
    particles: Vec<Particle>,
    emitted: i64,
    next_burst: f32,
    repetitions: u32,
    config: EffectConfig,
    beam: Option<beams::Beam>,
    stop_at: Option<f32>,
    ticks: u32,
    sprite: Option<sprites::SpriteEffect>,
    control: Option<control::Controller>,
    attractor: i32,
    composition: Option<composition::Composition>,
    mesh: Option<meshes::MeshEffect>,
    tracer_mesh: Option<tracer_meshes::TracerMesh>,
    aux: Option<aux::AuxEffect>,
    particle: Option<particles::ParticleEffect>,
    particle2: Option<particles2::ParticleEffect>,
    native_replicated: Option<replicated::ReplicatedEffect>,
    buff: Option<Box<buffs::Buff>>,
}

struct EffectModel {
    scene: Scene,
    uploaded: bool,
}

#[derive(Clone, Copy, Default)]
pub struct EffectConfig {
    pub duration: Option<f32>,
    pub repetitions: Option<u32>,
    /// RGBA, matching the renderer's vertex colour convention.
    pub start_color: Option<[f32; 4]>,
    pub stop_color: Option<[f32; 4]>,
    pub scale: Option<f32>,
    pub source_identity: Option<(u32,u32)>,
    pub target_identity: Option<(u32,u32)>,
    /// Nonzero CreateEffect2 dynel argument overrides the native connector attractor.
    pub source_attractor: Option<i32>,
    pub source_appearance: Option<[i32;4]>,
    pub target_appearance: Option<[i32;4]>,
    /// Native dynel locator overload; matrix-created children leave this false.
    pub track_source: bool,
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
    beam_state: beams::BeamState,
    random: R250,
    display_random: R250,
    mesh_resources: HashMap<u32,std::sync::Arc<Scene>>,
    mesh_uploaded: HashMap<u32,bool>,
    bph_last: HashMap<(u32,u32),(u32,f32)>,
    elapsed: f32,
    smoke_wind: Vec3,
    spawning: Vec<i32>,
    anchors: HashMap<((u32,u32),i32),Option<Mat4>>,
    anchor_ids: Vec<i32>,
    control_work: Vec<(u32,control::Controller)>,
    composition_work: Vec<(u32,composition::Composition)>,
    native_work: Vec<(u32,replicated::ReplicatedEffect)>,
    buff_work: Vec<(u32,Box<buffs::Buff>)>,
    buff_models: HashMap<u32,EffectModel>,
    retired_buff_models: Vec<u64>,
    aux_sounds: Vec<aux::AuxSound>,
    vulcan_sequence:u64,
}

impl Renderer {
    pub fn open(dir: &Path) -> Result<Self> {
        let store = RecordStore::open(dir)?;
        let names = NameTable::load(&store)?;
        let templates = Templates::open(dir)?;
        let mut anchor_ids: Vec<_> = templates.by_id.values().filter_map(|t| t.words.get(7).map(|v| *v as i32)).chain(1000..=1018).chain(2000..=2023).chain([3000,3001]).collect();
        anchor_ids.sort_unstable();
        anchor_ids.dedup();
        Ok(Self { templates, store, names, models: HashMap::new(), active: vec![], rng: CrtRand::new(1), next_actor: ACTOR_BASE, generation: u64::MAX, beam_state:beams::BeamState::default(),bph_last:HashMap::new(),elapsed:0.0,smoke_wind:Vec3::ZERO,spawning:vec![],anchors:HashMap::new(),anchor_ids,control_work:vec![],composition_work:vec![],native_work:vec![],buff_work:vec![],buff_models:HashMap::new(),retired_buff_models:vec![],random:R250::new(0xe6f1),display_random:R250::new(0xe6f1),mesh_resources:HashMap::new(),mesh_uploaded:HashMap::new(),aux_sounds:vec![],vulcan_sequence:0 })
    }

    pub fn clear(&mut self) {
        let active = std::mem::take(&mut self.active);
        for mut a in active {
            if let Some(c) = &mut a.control { c.cancel(self); }
            if let Some(c) = &mut a.composition { if let Err(error)=c.cancel(self) { eprintln!("effect cancellation: {error:#}"); } }
            if let Some(native)=&mut a.native_replicated { native.cancel(self); }
            if let Some(buff)=&mut a.buff { if let Err(error)=buff.cancel(self) {eprintln!("buff cancellation: {error:#}");} }
        }
        self.active.clear();
        self.retired_buff_models.extend(self.buff_models.iter().filter(|(_,model)|model.uploaded).map(|(&handle,_)|0xfac2_0000_0000_0000|u64::from(handle)));
        self.buff_models.clear();
        self.bph_last.clear();
        self.anchors.clear();
        self.aux_sounds.clear();
    }

    pub fn supports(kind: i32) -> bool { matches!(kind,1001|1005|1006|1010|1025|1027|1029|3025) || beams::Beam::supports(kind) || sprites::SpriteEffect::supports(kind) || composition::Composition::supports(kind) || aux::AuxEffect::supports(kind) || particles::ParticleEffect::supports(kind) || particles2::ParticleEffect::supports(kind) || replicated::ReplicatedEffect::supports(kind) || buffs::Buff::supports(kind) }
    pub fn delete(&mut self, handle: u32) {
        if let Some(index) = self.active.iter().position(|a| a.actor == handle) {
            let mut a = self.active.swap_remove(index);
            if let Some(c) = &mut a.control { c.cancel(self); }
            if let Some(c) = &mut a.composition { if let Err(error)=c.cancel(self) { eprintln!("effect cancellation: {error:#}"); } }
            if let Some(native)=&mut a.native_replicated { native.cancel(self); }
            if let Some(buff)=&mut a.buff { if let Err(error)=buff.cancel(self) {eprintln!("buff cancellation: {error:#}");} }
            if self.buff_models.remove(&handle).is_some_and(|model|model.uploaded) {self.retired_buff_models.push(0xfac2_0000_0000_0000|u64::from(handle));}
        }
    }
    pub fn source_model_changed(&mut self,identity:(u32,u32)) {
        for buff in self.active.iter_mut().filter_map(|a|a.buff.as_mut()) {
            if buff.identity()==Some(identity) {buff.source_model_changed();}
        }
    }
    pub fn is_active(&self, handle: u32) -> bool { self.active.iter().any(|a| a.actor == handle) }
    /// Destroyed source dynels delete Shield2/Trail2 and attached VulcanRocks.
    /// Lack of a visible actor or a fresh CPU pose is not dynel destruction.
    pub fn source_deleted(&mut self,identity:(u32,u32)) {
        while let Some(handle)=self.active.iter().find(|a| {
            a.config.source_identity==Some(identity)
                && (matches!(self.templates.by_id[&a.effect].kind,3034|3039)
                    || (self.templates.by_id[&a.effect].kind==1029 && self.templates.by_id[&a.effect].words[0]&0x400==0))
        }).map(|a|a.actor) {self.delete(handle);}
    }
    pub fn update_source(&mut self, handle: u32, source: Mat4) {
        let mut forwarded=None;
        if let Some(a) = self.active.iter_mut().find(|a| a.actor == handle) {
            let template=&self.templates.by_id[&a.effect];
            a.raw_source=source;
            a.source=if matches!(template.kind,1001|1004|1005|1006|1007|1008|1009|1012|1018|3020) {
                match sprites::connector(template,source) { Ok(m)=>m,Err(error)=>{eprintln!("effect connector: {error:#}");return;} }
            } else {source};
            if let Some(s) = &mut a.sprite { s.update_source(a.source); }
            if let Some(c) = &mut a.composition { c.update_source(source);forwarded=c.forwarded_children(); }
            if let Some(c) = &mut a.control { c.update_source(a.source); }
            if let Some(p)=&mut a.particle {p.update_source(a.source);}
            if let Some(p)=&mut a.particle2 {if let Err(error)=p.update_source(source) {eprintln!("effect particle connector: {error:#}");}}
            if let Some(mesh)=&mut a.mesh {if let Err(error)=mesh.update_source(source) {eprintln!("rock connector: {error:#}");}}
            if let Some(t)=&mut a.tracer_mesh { if let Err(error)=t.set_anchors(source,a.target) {eprintln!("effect mesh connector: {error:#}");} }
            if let Some(aux)=&mut a.aux { aux.update_source(source); }
            if let Some(native)=&mut a.native_replicated { native.update_source(source); }
            if let Some(buff)=&mut a.buff {if let Err(error)=buff.update_source(source) {eprintln!("buff source: {error:#}");}}
            if template.kind==1019 {
                if let Some(b)=&mut a.beam { if let Err(error)=b.update_source(source) {eprintln!("effect connector: {error:#}");} }
            }
        }
        if let Some(children)=forwarded { for child in children {if child!=0 {self.update_source(child,source);}} }
    }

    pub fn update_position(&mut self, handle:u32,position:Vec3) {
        let mut forwarded=None;
        let mut source=None;
        if let Some(a) = self.active.iter_mut().find(|a| a.actor==handle) {
            if let Some(c) = &mut a.composition { c.update_position(position);forwarded=c.forwarded_children(); }
            else if let Some(native)=&mut a.native_replicated {native.update_position(position);}
            else {let mut m=a.raw_source;m.w_axis=position.extend(1.0);source=Some(m);}
        }
        if let Some(source)=source {self.update_source(handle,source);}
        if let Some(children)=forwarded {for child in children {if child!=0 {self.update_position(child,position);}}}
    }

    pub fn prepare_anchors(&mut self,identity:(u32,u32),mut resolve:impl FnMut((u32,u32),i32)->Option<Mat4>) {
        for &id in &self.anchor_ids { self.anchors.insert((identity,id),resolve(identity,id)); }
    }
    pub fn prepare_anchor(&mut self, identity: (u32,u32), id: i32, matrix: Option<Mat4>) {
        self.anchors.insert((identity,id),matrix);
    }

    pub fn refresh_anchors(&mut self, mut resolve: impl FnMut((u32,u32),i32)->Option<Mat4>) {
        self.anchors.clear();
        for a in &self.active {
            if let (Some(identity),Some(id)) = (a.config.source_identity,a.config.source_attractor) {
                self.anchors.entry((identity,id)).or_insert_with(|| resolve(identity,id));
            }
            for identity in [a.config.source_identity,a.config.target_identity].into_iter().flatten() {
                for &id in &self.anchor_ids {
                    self.anchors.entry((identity,id)).or_insert_with(|| resolve(identity,id));
                }
            }
        }
        for a in &mut self.active {
            if let (Some(native),Some(identity)) = (&mut a.native_replicated,a.config.target_identity) {
                if self.templates.by_id[&a.effect].words[0]&1 !=0 {
                    if let Some(target)=resolve(identity,native.target_attractor()) {
                        a.target=target.w_axis.truncate();
                        native.update_target(target);
                    } else {native.invalidate_target();}
                }
            }
            let Some(identity) = a.config.source_identity else { continue };
            let template=&self.templates.by_id[&a.effect];
            if a.config.track_source && !matches!(template.kind,2007|2011|3004|3029|4000) && template.words[0]&1 !=0 && a.beam.is_none() && (a.mesh.is_none() || template.kind==1029) {
                if let Some(source) = self.anchors.get(&(identity,a.attractor)).copied().flatten() {
                    a.raw_source=source;
                    let source=if matches!(template.kind,1001|1004|1005|1006|1007|1008|1009|1012|1018|3020) {
                        match sprites::connector(template,source) {Ok(m)=>m,Err(error)=>{eprintln!("effect connector: {error:#}");continue;}}
                    } else {source};
                    a.source = source;
                    if let Some(s) = &mut a.sprite { s.update_source(source); }
                    if let Some(c) = &mut a.control { c.update_source(source); }
                    if let Some(c) = &mut a.composition { c.update_source(source); }
                    if let Some(p)=&mut a.particle {p.update_source(source);}
                    if let Some(p)=&mut a.particle2 {if let Err(error)=p.update_source(a.raw_source) {eprintln!("effect particle connector: {error:#}");}}
                    if let Some(mesh)=&mut a.mesh {if let Err(error)=mesh.update_source(source) {eprintln!("rock connector: {error:#}");}}
                    if let Some(t)=&mut a.tracer_mesh { if let Err(error)=t.set_anchors(source,a.target) {eprintln!("effect mesh connector: {error:#}");} }
                    if let Some(aux)=&mut a.aux {aux.update_source(source);}
                    if let Some(buff)=&mut a.buff {if let Err(error)=buff.update_source(source) {eprintln!("buff source: {error:#}");}}
                }
            }
            if let Some(c) = &mut a.control {
                if template.kind == 1010 {
                    let ids = c.anchor_ids();
                    let get = |who,id| self.anchors.get(&(who,id)).copied().flatten();
                    let hands = [get(identity,ids[0]),get(identity,ids[1])];
                    let target = a.config.target_identity.and_then(|who| get(who,ids[2]).or_else(|| get(who,2002).map(|m| Mat4::from_translation(Vec3::Y)*m)));
                    if let [Some(left),Some(right)] = hands {
                        c.update_anchors(&[left,right]);
                    }
                    if let Some(target) = target {
                        a.target = target.w_axis.truncate();
                        c.update_target(a.target);
                    }
                }
            }
            if let Some(buff)=&mut a.buff {
                if let Err(error)=buff.refresh_anchors(|id|self.anchors.get(&(identity,id)).copied().flatten()) {eprintln!("buff anchors: {error:#}");}
            }
            if let Some(s) = &mut a.sprite {
                let mut matrices = [Mat4::IDENTITY;14];
                let ids = s.anchor_ids();
                if ids.len() <= matrices.len() && ids.iter().enumerate().all(|(i,id)| {
                    if let Some(m) = self.anchors.get(&(identity,*id)).copied().flatten() { matrices[i]=m;true } else { false }
                }) { if let Err(error)=s.update_anchors(&matrices[..ids.len()]) {eprintln!("effect anchors: {error:#}");} }
            }
        }
    }
    pub fn terminate_gracefully(&mut self, handle: u32) {
        let Some(index) = self.active.iter().position(|a| a.actor == handle) else { return };
        let kind = self.templates.by_id[&self.active[index].effect].kind;
        // Native slot 6 sets the control's terminating byte for these classes:
        // 100d385e / 100f20bd / 100f5a3c / base 100a719a.
        if matches!(kind,1001|1010|1011|1012|1029|3007|3020|3031|3032) {
            self.delete(handle);
            return;
        }
        if let Some(mut buff)=self.active[index].buff.take() {
            if let Err(error)=buff.graceful(self) {eprintln!("buff termination: {error:#}");}
            if let Some(a)=self.active.iter_mut().find(|a|a.actor==handle) {a.buff=Some(buff);}
            return;
        }
        let mut children = None;
        let a = &mut self.active[index];
        let t = &self.templates.by_id[&a.effect];
        if matches!(kind,1005|1006) {
            let life = if kind == 1006 { a.config.duration.map(|d| d*0.8).unwrap_or(f32::from_bits(t.words[34])) } else { f32::from_bits(t.words[35]) };
            a.stop_at = Some(a.elapsed+life);
        } else if matches!(kind,1009|1018) {
            // GC100f0352 / 100f0f33: stop emission, retain the native lifetime tail.
            let life=f32::from_bits(t.words[if kind==1018 {35}else{26}]);
            if let Some(sprite) = &mut a.sprite { sprite.configure(EffectConfig {duration:Some(a.elapsed+life),..a.config}); }
            a.stop_at = Some(a.elapsed+life);
        } else if kind == 3028 {
            if let Some(particle) = &mut a.particle2 { particle.terminate_gracefully(); }
        } else if kind == 2007 {
            // GC100e5888 forwards the call without deleting the child handles.
            children = a.composition.as_ref().and_then(|c| c.forwarded_children());
        }
        if let Some(children) = children {
            for child in children { if child != 0 { self.terminate_gracefully(child); } }
        }
    }
    pub fn next_state(&mut self, handle: u32) {
        if let Some(a) = self.active.iter_mut().find(|a| a.actor == handle) {
            if let Some(c) = &mut a.control { c.next_state(); }
            if let Some(c) = &mut a.composition { c.next_state(); }
            if let Some(aux)=&mut a.aux {aux.next_state();}
        }
    }

    pub fn effect_kind(&self, effect: i32) -> Option<i32> {
        self.templates.by_id.get(&effect).map(|template| template.kind)
    }


    /// Default connector attractor (parameter 7), overridden by a nonzero item tuple.
    pub fn attractor(&self, effect: i32, explicit: i32) -> Option<i32> {
        if explicit != 0 { return Some(explicit); }
        let template=self.templates.by_id.get(&effect)?;
        if matches!(template.kind,2007|2011|3004|3029|4000) {Some(0)} else {template.words.get(7).map(|&v|v as i32)}
    }

    pub fn spawn(&mut self, binding: Binding, source: Mat4, target: Vec3) -> Result<()> {
        self.spawn_configured(binding, source, target, EffectConfig::default()).map(|_| ())
    }

    pub fn spawn_configured(&mut self, binding: Binding, source: Mat4, target: Vec3, config: EffectConfig) -> Result<u32> {
        ensure!(self.spawning.len() < 64 && !self.spawning.contains(&binding.effect), "cyclic or too deep effect composition {}",binding.effect);
        self.spawning.push(binding.effect);
        let result = self.spawn_inner(binding,source,target,config).with_context(|| format!("authored effect {}",binding.effect));
        self.spawning.pop();
        result
    }

    fn spawn_inner(&mut self, binding: Binding, source: Mat4, target: Vec3, config: EffectConfig) -> Result<u32> {
        if binding.effect == 49999 { return Ok(0); }
        ensure!(source.is_finite() && target.is_finite(), "nonfinite effect anchor");
        for value in [config.duration, config.scale].into_iter().flatten() {
            ensure!(value.is_finite(), "nonfinite effect configuration");
        }
        for color in [config.start_color, config.stop_color].into_iter().flatten() {
            ensure!(color.iter().all(|c| c.is_finite()), "nonfinite effect color");
        }
        let template = self.templates.by_id.get(&binding.effect).with_context(|| format!("missing effect {}", binding.effect))?;
        ensure!(Self::supports(template.kind), "unsupported authored weapon effect {} class {}", binding.effect, template.kind);
        if template.kind == 1006 {
            ensure!(config.duration.is_none_or(|d| d > 0.0), "invalid starburst duration");
        }
        let attractor = if let Some(explicit) = config.source_attractor.filter(|&id| id != 0) { explicit } else if binding.attractor != 0 { binding.attractor } else if matches!(template.kind,2007|2011|3004|3029|4000) {0} else {template.word(7)? as i32};
        let source = if config.track_source && !matches!(template.kind,2007|2011|3004|3029|4000) {
            let who=config.source_identity.context("dynel effect requires source identity")?;
            self.anchors.get(&(who,attractor)).copied().flatten().with_context(|| format!("missing effect {} source anchor {attractor}",binding.effect))?
        } else {source};
        let raw_source=source;
        let source = if matches!(template.kind,1001|1004|1005|1006|1007|1008|1009|1012|1018|3020) { sprites::connector(template,source)? } else { source };
        let controller = if matches!(template.kind,1001|1010) { Some(control::Controller::new(template,source,target,config)?) } else { None };
        let mut sprite = if sprites::SpriteEffect::supports(template.kind) { Some(sprites::SpriteEffect::new_with_id(binding.effect,template,source,Mat4::from_translation(target),binding.color,&mut self.display_random)?) } else { None };
        if let Some(s) = &mut sprite { s.configure(config); }
        let mut composition = if composition::Composition::supports(template.kind) { Some(composition::Composition::new(template,source,target,config)?) } else { None };
        let mut beam = if beams::Beam::supports(template.kind) { Some(beams::Beam::new(template,source,Mat4::from_translation(target),binding.color)?) } else { None };
        if let (Some(beam),Some(duration)) = (&mut beam,config.duration) { beam.set_duration(duration); }
        let mut mesh = if matches!(template.kind,1027|1029) { Some(meshes::MeshEffect::new(template,&self.store,&self.names,source,target,&mut self.mesh_resources)?) } else { None };
        if let (Some(mesh),Some(duration)) = (&mut mesh,config.duration) { mesh.set_duration(duration); }
        if template.kind==1029 {
            if let Some(mesh)=&mut mesh {mesh.sequence=self.vulcan_sequence;}
            self.vulcan_sequence+=1;
        }
        if let Some(mesh) = &mesh { for (selector,_) in mesh.scenes() { self.mesh_uploaded.entry(*selector).or_insert(false); } }
        let tracer_mesh=if template.kind==3025 {Some(tracer_meshes::TracerMesh::new(template,&self.store,&self.names,source,&mut self.mesh_resources,config)?)} else {None};
        if let Some(mesh)=&tracer_mesh {for (selector,_) in mesh.scenes() {self.mesh_uploaded.entry(*selector).or_insert(false);}}
        let auxiliary=if aux::AuxEffect::supports(template.kind) {Some(aux::AuxEffect::new(template,source,target,config,&self.store)?)} else {None};
        let mut particle=if particles::ParticleEffect::supports(template.kind) {Some(particles::ParticleEffect::new(template,source,Mat4::from_translation(target),binding.color,&mut self.random,&mut self.display_random,&mut self.rng)?)} else {None};
        if let Some(p)=&mut particle {p.configure(config)?;}
        let mut particle2=if particles2::ParticleEffect::supports(template.kind) {Some(particles2::ParticleEffect::new(template,raw_source,Mat4::from_translation(target),binding.color,&mut self.random,&mut self.display_random,&mut self.rng)?)} else {None};
        if let Some(p)=&mut particle2 {p.configure(config)?;}
        let mut native_replicated=if replicated::ReplicatedEffect::supports(template.kind) {
            let mut native_config=config;
            if binding.color!=0 {
                let [a,r,g,b]=binding.color.to_be_bytes().map(|v| v as f32/255.0);
                native_config.start_color=Some([r,g,b,a]);
                native_config.stop_color=Some([r,g,b,0.0]);
            }
            let mut native=replicated::ReplicatedEffect::new(template,raw_source,target,native_config)?;
            if let Some(identity)=config.target_identity {
                let matrix=self.anchors.get(&(identity,native.target_attractor())).copied().flatten()
                    .context("missing replicated effect target connector")?;
                native.update_target(matrix);
            }
            Some(native)
        } else {None};
        let mut buff=if buffs::Buff::supports(template.kind) {Some(Box::new(buffs::Buff::new(template,raw_source,target,binding.color,config,&mut self.random,&mut self.display_random,&mut self.rng)?))} else {None};
        if let (Some(buff),Some(identity))=(&mut buff,config.source_identity) {
            buff.refresh_anchors(|id|self.anchors.get(&(identity,id)).copied().flatten())?;
        }
        let count = if template.kind == 1025 || beam.is_some() || sprite.is_some() || controller.is_some() || composition.is_some() || mesh.is_some() || tracer_mesh.is_some() || auxiliary.is_some() || particle.is_some() || particle2.is_some() || native_replicated.is_some() || buff.is_some() { 1 } else { sprite_capacity(template)? };
        if buff.is_none() && mesh.is_none() && tracer_mesh.is_none() && auxiliary.is_none() && !self.models.contains_key(&binding.effect) {
            let groups = if let Some(beam) = &beam { beam.models() } else if let Some(s) = &sprite { s.models() } else if let Some(c) = &controller { c.models() } else if let Some(c) = &composition { c.models() } else if let Some(p)=&particle {p.models()} else if let Some(p)=&particle2 {p.models()} else if let Some(native)=&native_replicated {native.models()} else {
                vec![(Some(template.word(9)? as usize), (0..count as u32).flat_map(|i| [i*4,i*4+2,i*4+3,i*4,i*4+3,i*4+1]).collect(), count*4)]
            };
            let blends = if let Some(beam) = &beam { beam.blends() } else if let Some(s) = &sprite { s.blends() } else if let Some(c) = &controller { c.blends() } else if let Some(c) = &composition { c.blends() } else if let Some(p)=&particle {p.blends()} else if let Some(p)=&particle2 {p.blends()} else if let Some(native)=&native_replicated {native.blends()} else { vec![Blend::Additive] };
            let mut scene = Scene::default();
            let mut vertices = Vec::new();
            let mut submeshes = Vec::new();
            for (group,((material, indices, n), blend)) in groups.into_iter().zip(blends).enumerate() {
                let key = if let Some(key) = beam.as_ref().and_then(|b| b.texture_override(group)) { Some(key) }
                else if let Some(material) = material {
                    let &(name,_,_,_,_) = materials::MATERIALS.get(material).context("unknown effect material")?;
                    Some(TextureKey { rdb_type:1010004, id:self.names.id(1010004,name).with_context(|| format!("missing effect texture {name}"))? })
                } else { None };
                if let Some(key) = key {
                    if let std::collections::hash_map::Entry::Vacant(entry) = scene.textures.entry(key) {
                        let texture = ao_formats::texture::load_texture(&self.store,key)?.with_context(|| format!("missing effect texture {}",key.id))?;
                        entry.insert(texture);
                    }
                }
                let offset = vertices.len() as u32;
                let mut sub = Submesh::new(indices.into_iter().map(|i| i+offset).collect(),key);
                sub.blend = blend;
                sub.two_sided = true;
                sub.emissive = [1.0;3];
                submeshes.push(sub);
                vertices.resize(vertices.len()+n,Vertex::default());
            }
            scene.meshes.push(Mesh { vertices, submeshes });
            self.models.insert(binding.effect,EffectModel { scene,uploaded:false });
        }
        let particles = Vec::with_capacity(count);
        let repetitions = config.repetitions.unwrap_or(if template.kind == 1006 { template.word(37)?.max(1) } else { 0 });
        let emitted = if template.kind == 1005 { -(template.word(31)? as i64) } else { 0 };
        let actor = self.next_actor;
        self.next_actor = self.next_actor.wrapping_add(mesh.as_ref().map(|m| m.actor_count()).or_else(|| tracer_mesh.as_ref().map(|m|m.actor_count())).unwrap_or(1).max(1) as u32).max(ACTOR_BASE);
        if let Some(c) = &mut composition { c.initialize(self)?; }
        if let Some(native)=&mut native_replicated { native.initialize(self)?; }
        if let Some(buff)=&buff {
            if buff.needs_private_model() {let model=self.buff_model(buff)?;self.buff_models.insert(actor,model);}
            else if !self.models.contains_key(&binding.effect) {let model=self.buff_model(buff)?;self.models.insert(binding.effect,model);}
        }
        self.active.push(Active { actor, effect: binding.effect, source, raw_source,target, elapsed: 0.0, color: binding.color, particles, emitted, next_burst: -1.0, repetitions, config, beam, stop_at: None, ticks:0,sprite,control:controller,attractor,composition,mesh,tracer_mesh,aux:auxiliary,particle,particle2,native_replicated,buff });
        Ok(actor)
    }
    fn buff_model(&self,buff:&buffs::Buff)->Result<EffectModel> {
        let mut scene=Scene::default();
        let mut vertices=Vec::new();
        let mut submeshes=Vec::new();
        for ((material,indices,count),blend) in buff.models().into_iter().zip(buff.blends()) {
            let key=if let Some(key)=buff.source_texture() {Some(key)} else if let Some(material)=material {
                let &(name,_,_,_,_)=materials::MATERIALS.get(material).context("unknown buff material")?;
                Some(TextureKey {rdb_type:1010004,id:self.names.id(1010004,name).with_context(||format!("missing buff texture {name}"))?})
            } else {None};
            if let Some(key)=key {
                if let std::collections::hash_map::Entry::Vacant(entry)=scene.textures.entry(key) {
                    entry.insert(ao_formats::texture::load_texture(&self.store,key)?.with_context(||format!("missing buff texture {}",key.id))?);
                }
            }
            let offset=vertices.len() as u32;
            let mut sub=Submesh::new(indices.into_iter().map(|i|i+offset).collect(),key);
            sub.blend=blend;sub.two_sided=true;sub.emissive=[1.0;3];
            submeshes.push(sub);vertices.resize(vertices.len()+count,Vertex::default());
        }
        scene.meshes.push(Mesh {vertices,submeshes});
        Ok(EffectModel {scene,uploaded:false})
    }
    pub fn needs_source_mesh(&self,identity:(u32,u32))->bool {
        self.active.iter().filter_map(|a|a.buff.as_ref()).any(|b|b.identity()==Some(identity)&&b.needs_source_mesh())
    }
    pub fn needs_source_pose(&self,identity:(u32,u32))->bool {
        self.active.iter().filter_map(|a|a.buff.as_ref()).any(|b|b.identity()==Some(identity)&&b.needs_source_pose())
    }
    pub fn prepare_source_mesh(&mut self,identity:(u32,u32),scene:&Scene,actor:&ActorFrame) {
        for index in 0..self.active.len() {
            let Some(mut buff)=self.active[index].buff.take() else {continue};
            if buff.identity()==Some(identity)&&buff.needs_source_mesh() {
                match buff.prepare_source(scene,actor,&mut self.rng) {
                    Ok(true) if buff.needs_private_model()=>match self.buff_model(&buff) {
                        Ok(model)=>{self.buff_models.insert(self.active[index].actor,model);}
                        Err(error)=>eprintln!("buff model: {error:#}"),
                    },
                    Ok(_)=>{},
                    Err(error)=>eprintln!("buff source geometry: {error:#}"),
                }
            }
            self.active[index].buff=Some(buff);
        }
    }

    pub fn take_aux_sounds(&mut self)->impl Iterator<Item=AuxSound>+'_ {self.aux_sounds.drain(..)}
    pub fn camera_offset(&mut self,eye:Vec3)->Result<Vec3> {
        let mut offset=Vec3::ZERO;
        for effect in &self.active {
            if let Some(aux)=&effect.aux {
                if let Some(value)=aux.camera_offset(eye,&mut self.random)? {offset=value;}
            }
        }
        Ok(offset)
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

    pub fn frame(&mut self, dt: f32, host: &mut ao_render::Host, mut collision:Option<&mut dyn FnMut(Vec3)->Option<(Vec3,Vec3)>>) {
        if !dt.is_finite() { return; }
        self.elapsed += dt.abs();
        let wind = Vec3::from_array(host.effect_wind);
        self.smoke_wind = self.smoke_wind*0.96+wind*0.04;
        let mut work = std::mem::take(&mut self.control_work);
        for a in &mut self.active {
            if let Some(c) = a.control.take() { work.push((a.actor,c)); }
        }
        for (handle,mut c) in work.drain(..) {
            if !self.is_active(handle) { c.cancel(self);continue; }
            let alive = match c.frame(dt.abs(),self) { Ok(alive) => alive,Err(error) => { eprintln!("effect control: {error:#}");c.cancel(self);false } };
            if alive {
                if let Some(a) = self.active.iter_mut().find(|a| a.actor==handle) { a.control=Some(c); }
                else { c.cancel(self); }
            } else { self.delete(handle); }
        }
        self.control_work = work;
        let mut work = std::mem::take(&mut self.composition_work);
        for a in &mut self.active {
            if let Some(c) = a.composition.take() { work.push((a.actor,c)); }
        }
        for (handle,mut c) in work.drain(..) {
            if !self.is_active(handle) { if let Err(error)=c.cancel(self) { eprintln!("effect cancellation: {error:#}"); } continue; }
            let alive = match c.frame(dt.abs(),self) { Ok(alive) => alive,Err(error) => { eprintln!("effect composition: {error:#}");if let Err(error)=c.cancel(self) { eprintln!("effect cancellation: {error:#}"); } false } };
            if alive {
                if let Some(a) = self.active.iter_mut().find(|a| a.actor==handle) { a.composition=Some(c); }
                else if let Err(error)=c.cancel(self) { eprintln!("effect cancellation: {error:#}"); }
            } else { self.delete(handle); }
        }
        self.composition_work = work;
        let mut work=std::mem::take(&mut self.native_work);
        for a in &mut self.active {
            if let Some(native)=a.native_replicated.take() { work.push((a.actor,native)); }
        }
        for (handle,mut native) in work.drain(..) {
            if !self.is_active(handle) { native.cancel(self); continue; }
            let alive=match native.frame(dt.abs(),self) {
                Ok(alive)=>alive,
                Err(error)=>{eprintln!("replicated effect: {error:#}");false}
            };
            if alive {
                if let Some(a)=self.active.iter_mut().find(|a| a.actor==handle) {a.native_replicated=Some(native);}
                else {native.cancel(self);}
            } else {native.cancel(self);self.delete(handle);}
        }
        self.native_work=work;
        let mut work=std::mem::take(&mut self.buff_work);
        for a in &mut self.active {if let Some(buff)=a.buff.take() {work.push((a.actor,buff));}}
        for (handle,mut buff) in work.drain(..) {
            if !self.is_active(handle) {let _=buff.cancel(self);continue;}
            let alive=match buff.frame(dt.abs(),self) {
                Ok(alive)=>alive,
                Err(error)=>{eprintln!("buff control: {error:#}");false},
            };
            if alive {
                if let Some(a)=self.active.iter_mut().find(|a|a.actor==handle) {a.buff=Some(buff);}
                else {let _=buff.cancel(self);}
            } else {let _=buff.cancel(self);self.delete(handle);}
        }
        self.buff_work=work;
        if self.generation != host.scene_generation() {
            self.generation = host.scene_generation();
            for model in self.models.values_mut() { model.uploaded = false; }
            for model in self.buff_models.values_mut() {model.uploaded=false;}
            for uploaded in self.mesh_uploaded.values_mut() { *uploaded=false; }
        }
        for (handle,model) in &mut self.buff_models {
            if !model.uploaded {
                host.actor_models.push((0xfac2_0000_0000_0000|u64::from(*handle),model.scene.clone()));
                model.uploaded=true;
            }
        }
        for (id, model) in &mut self.models {
            if !model.uploaded {
                host.actor_models.push((MODEL_BASE | *id as u32 as u64, model.scene.clone()));
                model.uploaded = true;
            }
        }
        for (selector,uploaded) in &mut self.mesh_uploaded {
            if !*uploaded {
                host.actor_models.push((MESH_MODEL_BASE|u64::from(*selector),self.mesh_resources[selector].as_ref().clone()));
                *uploaded=true;
            }
        }
        let right = host.camera.right();
        let up = host.camera.up();
        let camera = host.camera.pos;
        let forward = host.camera.forward();
        let mut live_rocks=self.active.iter().filter_map(|a|a.mesh.as_ref()).map(meshes::MeshEffect::rock_count).sum::<usize>();
        self.active.retain_mut(|effect| {
            let template = &self.templates.by_id[&effect.effect];
            if !dt.is_finite() { return true; }
            // GC100d2531 initializes at zero; first advancing step is capped at .033.
            let step = match effect.ticks { 0 => 0.0,1 => dt.abs().min(0.033),_ => dt.abs() };
            effect.ticks = effect.ticks.saturating_add(1);
            effect.elapsed += step;
            if let Some(mesh)=&mut effect.mesh {
                if template.kind==1029 && mesh.sequence+50<self.vulcan_sequence {return false;}
                let Some(terrain)=collision.as_deref_mut() else { eprintln!("effect {} needs native terrain collision",effect.effect);return false; };
                match mesh.frame_with_pool(step,&mut self.random,terrain,&mut live_rocks) {
                    Ok(true)=>host.actors.extend(mesh.actors(effect.actor,MESH_MODEL_BASE)),
                    Ok(false)=>return false,
                    Err(error)=>{ eprintln!("effect mesh: {error:#}");return false; }
                }
                return true;
            }
            if let Some(mesh)=&mut effect.tracer_mesh {
                match mesh.frame(step) {
                    Ok(true)=>host.actors.extend(mesh.actors(effect.actor,MESH_MODEL_BASE)),
                    Ok(false)=>return false,
                    Err(error)=>{eprintln!("effect tracer mesh: {error:#}");return false;}
                }
                return true;
            }
            if let Some(aux)=&mut effect.aux {
                let alive=aux.advance(step);
                if let Some(sound)=aux.take_sound() {self.aux_sounds.push(sound);}
                return alive;
            }
            if let Some(buff)=&mut effect.buff {
                buff.apply_material(&mut host.actors);
                let terrain:Option<&mut dyn FnMut(Vec3)->Option<(Vec3,Vec3)>>=match &mut collision {Some(f)=>Some(&mut **f),None=>None};
                match buff.vertices(effect.elapsed,camera,right,up,&mut self.random,&mut self.display_random,&mut self.rng,terrain) {
                    Ok(Some(groups))=>{
                        let skin:Vec<_>=groups.into_iter().flatten().collect();
                        if !skin.is_empty() {
                            let model=if buff.needs_private_model() {0xfac2_0000_0000_0000|u64::from(effect.actor)}else{MODEL_BASE|effect.effect as u32 as u64};
                            host.actors.push(ActorFrame {id:effect.actor,model,transform:IDENTITY,skin:Some(skin),always:true,alpha:1.0,..Default::default()});
                        }
                        return true;
                    },
                    Ok(None)=>return false,
                    Err(error)=>{eprintln!("buff vertices: {error:#}");return false;},
                }
            }
            let duration = effect.stop_at.unwrap_or_else(|| if template.kind == 1006 { f32::from_bits(template.words[8]) } else { effect.config.duration.unwrap_or(f32::from_bits(template.words[8])) });
            if effect.control.is_none() && effect.composition.is_none() && effect.native_replicated.is_none() && effect.particle.is_none() && effect.particle2.is_none() && duration > 0.0 && effect.elapsed > duration { return false; }
            let mut skin = Vec::with_capacity(self.models[&effect.effect].scene.meshes[0].vertices.len());
            if let Some(beam) = &mut effect.beam {
                let Ok(Some(groups)) = beam.vertices(effect.elapsed,camera,right,up,&mut self.beam_state) else { return false };
                for group in groups { skin.extend(group); }
            } else if let Some(p)=&mut effect.particle {
                let Ok(Some(groups))=p.vertices(effect.elapsed,camera,right,up,&mut self.random,&mut self.display_random,&mut self.rng) else {return false;};
                for group in groups {skin.extend(group);}
            } else if let Some(p)=&mut effect.particle2 {
                let Ok(Some(groups))=p.vertices(effect.elapsed,camera,right,up,&mut self.random,&mut self.display_random,&mut self.rng) else {return false;};
                for group in groups {skin.extend(group);}
            } else if let Some(s) = &mut effect.sprite {
                s.update_wind(wind,self.smoke_wind);
                let Ok(Some(groups)) = s.vertices(effect.elapsed,camera,right,up,&mut self.random,&mut self.display_random,&mut self.rng) else { return false };
                for group in groups { skin.extend(group); }
            } else if let Some(c) = &mut effect.control {
                let Ok(Some(groups)) = c.vertices(effect.elapsed,camera,right,up) else { return false };
                for group in groups { skin.extend(group); }
                if skin.is_empty() { return true; }
            } else if let Some(c) = &mut effect.composition {
                let Ok(Some(groups)) = c.vertices(effect.elapsed,camera,right,up) else { return false };
                for group in groups { skin.extend(group); }
                if skin.is_empty() { return true; }
            } else if let Some(native)=&mut effect.native_replicated {
                if native.write_vertices(effect.elapsed,right,up,&mut self.display_random,&mut skin).is_err() {return false;}
                if skin.is_empty() {return true;}
            } else if template.kind == 1025 {
                let Ok(Some((tail, head, width))) = Self::projectile(template, effect.source.w_axis.truncate(), effect.target, effect.elapsed) else { return false };
                let side = (head-tail).cross(camera-(head+tail)*0.5).normalize_or_zero()*width;
                let color = tint(template, effect.color, 0.0, 13);
                quad(&mut skin, [tail-side, tail+side, head-side, head+side], [[0.0,0.0], [0.0,1.0], [1.0,0.0], [1.0,1.0]], color);
            } else {
                let capacity = self.models[&effect.effect].scene.meshes[0].vertices.len()/4;
                effect.particles.retain(|p| effect.elapsed-p.born <= p.life);
                emit_sprites(template, effect, &mut self.rng, &mut self.random, capacity, step);
                let material = template.words[9] as usize;
                let (_, columns, rows, first, last) = materials::MATERIALS[material];
                let mut alive = false;
                for p in &effect.particles {
                    let age = effect.elapsed-p.born;
                    let t = (age/p.life).min(1.0);
                    let live = age <= p.life;
                    alive |= live;
                    let radius = lerp(template, 12, 13, t)*p.radius_scale;
                    let transform = if template.words[0]&2 != 0 { effect.source } else { Mat4::IDENTITY };
                    let p_world = transform.transform_point3(p.position+p.velocity_p*age);
                    let q_world = transform.transform_point3(p.position+p.velocity_q*age);
                    let view = |p: Vec3| { let d=p-camera; glam::Vec2::new(d.dot(right),d.dot(up))/d.dot(forward) };
                    let d = (view(q_world)-view(p_world)).normalize_or_zero();
                    let (a,b) = if d == glam::Vec2::ZERO { (right*radius,up*radius) } else { ((right*d.x+up*d.y)*radius,(right*d.y-up*d.x)*radius) };
                    let frame = (first as f32+(last-first) as f32*t) as u32;
                    // DS1001364b divides by rows and takes remainder by columns.
                    let (x,y) = (frame%columns, frame/rows);
                    let uv = [[x as f32/columns as f32,(y+1) as f32/rows as f32], [x as f32/columns as f32,y as f32/rows as f32], [(x+1) as f32/columns as f32,(y+1) as f32/rows as f32], [(x+1) as f32/columns as f32,y as f32/rows as f32]];
                    let mut color = tint(template, effect.color, t, 16);
                    if effect.config.start_color.is_some() || effect.config.stop_color.is_some() {
                        let native = |offset| [f32::from_bits(template.words[offset+1]),f32::from_bits(template.words[offset+2]),f32::from_bits(template.words[offset+3]),f32::from_bits(template.words[offset])];
                        let start = effect.config.start_color.unwrap_or_else(|| native(16));
                        let stop = effect.config.stop_color.unwrap_or_else(|| native(20));
                        color = std::array::from_fn(|i| start[i]+(stop[i]-start[i])*t);
                        for c in &mut color[..3] { *c = c.max(0.0).powf(2.2); }
                    }
                    if !live { color[3] = 0.0; }
                    quad(&mut skin, [p_world-a-b, p_world-a+b, q_world+a-b, q_world+a+b], uv, color);
                }
                while skin.len() < capacity*4 { skin.push(Vertex { color:[0.0;4],..Vertex::default() }); }
                if !alive && template.kind == 1005 && template.words[0]&0x200 != 0 { return false; }
            }
            // Dynamic world-space skin has no useful bind-pose sphere (actors.rs:191).
            host.actors.push(ActorFrame { id: effect.actor, model: MODEL_BASE | effect.effect as u32 as u64, transform: IDENTITY, parts: vec![], skin: Some(skin), always: true, alpha: 1.0, ..Default::default() });
            true
        });
        self.buff_models.retain(|handle,model| {
            let alive=self.active.iter().any(|a|a.actor==*handle);
            if !alive && model.uploaded {self.retired_buff_models.push(0xfac2_0000_0000_0000|u64::from(*handle));}
            alive
        });
        host.actor_models.retain(|(key,_)|!self.retired_buff_models.contains(key));
        host.actor_model_removals.append(&mut self.retired_buff_models);
    }
}

// GC100dcc70: capacity=max(initial count,trunc(rate*1.5*maximum life)).
fn sprite_capacity(template: &Template) -> Result<usize> {
    let end = if template.kind == 1006 { 38 } else { 36 };
    ensure!(template.words.len() >= end, "short sprite template");
    for i in 1..end {
        if !matches!(i, 7 | 9 | 24 | 31 | 37) { template.float(i)?; }
    }
    ensure!(template.float(34)? > 0.0 && template.float(35)? > 0.0, "invalid effect sprite lifetime");
    ensure!(template.float(10)? >= 0.0, "negative sprite emission rate");
    let count = (template.word(31)? as usize).max((template.float(10)? as f64*1.5*template.float(35)? as f64).trunc() as usize);
    ensure!((1..=4096).contains(&count), "invalid effect sprite capacity {count}");
    Ok(count)
}

/// GC1013d97c: signed integer conversion is rounded to float before unsigned correction.
pub(super) fn random_fraction(random:&mut R250)->f32 {
    let word=random.next_u32() as i32;
    let value=word as f32;
    (if word<0 {value+4294967296.0} else {value})*(1.0/4294967296.0)
}

fn sprite(template: &Template, rng: &mut R250, born: f32, dt: f32, star: Option<(f32, f32, f32)>) -> Particle {
    let f = |i| f32::from_bits(template.words[i]);
    let mut random = || random_fraction(rng);
    let (az, el, speed, life, radius_scale) = if let Some((az,el,speed)) = star {
        (az,el,speed,f(34),f(35))
    } else {
        (f(25)+(f(26)-f(25))*random(), f(27)+(f(28)-f(27))*random(), f(29)+(f(30)-f(29))*random(), f(34)+(f(35)-f(34))*random(), 1.0)
    };
    // GC100dcd93 native Z=-sin(el); effect_matrix conjugates the scene Z mirror,
    // so feed mirrored local Z into the already mirrored connector.
    let velocity = Vec3::new(az.cos()*el.cos(), az.sin()*el.cos(), el.sin())*speed;
    let (p,q) = if template.words[0]&0x100 == 0 { (f(32),f(33)) } else { (f(33),f(32)) };
    let instant = life < dt*2.0;
    Particle { position: if instant { velocity } else { Vec3::ZERO }, velocity_p: if instant { Vec3::ZERO } else { velocity*(p/life) }, velocity_q: if instant { Vec3::ZERO } else { velocity*(q/life) }, life, born, radius_scale }
}

// GC 100dd715: cumulative trunc(rate * elapsed), random bit mask w24,
// and emission stops one maximum lifetime before the control's duration.
fn emit_sprites(template: &Template, effect: &mut Active, rng: &mut CrtRand, random:&mut R250, capacity: usize, dt: f32) {
    let f = |i| f32::from_bits(template.words[i]);
    if template.kind == 1005 {
        let duration = effect.stop_at.or(effect.config.duration).unwrap_or(f(8));
        if rng.rand() & template.words[24] != 0 || (duration >= 0.0 && effect.elapsed >= duration-f(35)) { return; }
        let desired = (f(10)*effect.elapsed).trunc() as i64;
        let count = (desired-effect.emitted).max(0) as usize;
        effect.emitted = desired;
        for _ in 0..count.min(capacity-effect.particles.len()) {
            let p = locate_particle(sprite(template,random,effect.elapsed-dt,dt,None),template,effect.source);
            effect.particles.push(p);
        }
    } else if effect.particles.is_empty() {
        // GC 100de9fb: arm delay on an empty visual; only then generate next star.
        if effect.next_burst < 0.0 { effect.next_burst = effect.elapsed+effect.config.duration.map(|d| d*0.2).unwrap_or(f(36).max(0.0)); }
        else if effect.next_burst < effect.elapsed {
            effect.next_burst = -1.0;
            if effect.repetitions == 0 {
                effect.stop_at = Some(effect.elapsed+effect.config.duration.map(|d| d*0.8).unwrap_or(f(34)));
                return;
            }
            effect.repetitions -= 1;
            for quadrant in 0..4 {
                let az = quadrant as f32*std::f32::consts::FRAC_PI_2;
                let rings = if template.words[0]&0x1000 == 0 { [(0.5,0.62831855),(0.75,0.9424779),(1.0,1.2566371)] } else { [(0.75,0.0),(0.7,0.31415927),(1.0,1.335177)] };
                for (speed, elevation) in rings {
                    let (az,el) = if template.words[0]&0x800 != 0 {
                        let el = elevation+random_fraction(random)*0.4-0.2;
                        (az+random_fraction(random)*0.4-0.2,el)
                    } else { (az,elevation) };
                    if effect.particles.len() < capacity {
                        let mut p = sprite(template,random,effect.elapsed-dt,dt,Some((az,el,speed*f(35))));
                        configure_star_life(&mut p,effect.config.duration);
                        effect.particles.push(locate_particle(p,template,effect.source));
                    }
                }
            }
            if effect.particles.len() < capacity {
                let mut p = sprite(template,random,effect.elapsed-dt,dt,Some((std::f32::consts::TAU,std::f32::consts::FRAC_PI_2,2.0*f(35))));
                configure_star_life(&mut p,effect.config.duration);
                effect.particles.push(locate_particle(p,template,effect.source));
            }
        }
    }
}

// GC10105eb4/10105e83: bit2 chooses local particles under the live visual transform;
// without it, birth position and velocity are world-space and never follow later motion.
fn locate_particle(mut p:Particle,template:&Template,source:Mat4)->Particle {
    if template.words[0]&2 == 0 {
        p.position=source.transform_point3(p.position);
        p.velocity_p=source.transform_vector3(p.velocity_p);
        p.velocity_q=source.transform_vector3(p.velocity_q);
    }
    p
}

fn configure_star_life(p: &mut Particle, duration: Option<f32>) {
    if let Some(duration) = duration {
        let life = duration*0.8;
        p.velocity_p *= p.life/life;
        p.velocity_q *= p.life/life;
        p.life = life;
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
    fn authored_sprite(kind: i32) -> Template {
        let mut words = vec![0; if kind == 1006 { 38 } else { 36 }];
        for (i,value) in [(8,2.0f32),(10,10.0),(12,0.1),(13,0.2),(14,1.0),(15,1.0),(16,1.0),(17,1.0),(21,1.0),(30,1.0),(33,1.0),(34,0.2),(35,0.2)] { words[i] = value.to_bits(); }
        words[31] = if kind == 1006 { 16 } else { 2 };
        if kind == 1006 { words[37] = 1; }
        Template { kind, words }
    }

    fn active_sprite(template: &Template) -> Active {
        Active { actor:ACTOR_BASE,effect:1,source:Mat4::IDENTITY,raw_source:Mat4::IDENTITY,target:Vec3::X,elapsed:0.0,color:0,particles:vec![],emitted:-(template.words[31] as i64),next_burst:-1.0,repetitions:1,config:EffectConfig::default(),beam:None,stop_at:None,ticks:0,sprite:None,control:None,attractor:0,composition:None,mesh:None,tracer_mesh:None,aux:None,particle:None,particle2:None,native_replicated:None,buff:None }
    }

    #[test]
    fn flare_birth_space_does_not_drag_world_particles() {
        let mut t=authored_sprite(1005);
        let birth=Mat4::from_rotation_translation(glam::Quat::from_rotation_y(0.5),Vec3::X*5.0);
        let mut random=R250::new(0xe6f1);
        let p=locate_particle(sprite(&t,&mut random,0.0,0.0,None),&t,birth);
        assert_eq!(p.position,Vec3::X*5.0);
        t.words[0]|=2;
        let local=locate_particle(sprite(&t,&mut random,0.0,0.0,None),&t,birth);
        assert_eq!(local.position,Vec3::ZERO);
        let moved=Mat4::from_translation(Vec3::X*10.0);
        assert_eq!(moved.transform_point3(local.position),Vec3::X*10.0);
        assert_eq!(p.position,Vec3::X*5.0);
    }

    #[test]
    fn sprite_emission_uses_cumulative_rate_duration_and_bounded_capacity() {
        let mut t = authored_sprite(1005);
        assert_eq!(sprite_capacity(&t).unwrap(),3,"native 1.5 lifetime reserve");
        let mut a = active_sprite(&t);
        let mut rng = CrtRand::new(1);
        let mut random = R250::new(0xe6f1);
        emit_sprites(&t,&mut a,&mut rng,&mut random,2,0.0);
        assert_eq!(a.particles.len(),2);
        a.particles.clear();
        a.elapsed = 0.35;
        emit_sprites(&t,&mut a,&mut rng,&mut random,2,0.05);
        assert_eq!(a.emitted,3);
        assert_eq!(a.particles.len(),2);
        a.particles.clear();
        a.elapsed = 1.8;
        emit_sprites(&t,&mut a,&mut rng,&mut random,2,0.05);
        assert!(a.particles.is_empty());
        t.words[10] = f32::INFINITY.to_bits();
        assert!(sprite_capacity(&t).is_err());
        t.words[10] = 0;
        t.words[31] = u32::MAX;
        assert!(sprite_capacity(&t).is_err());
        t.words.truncate(20);
        assert!(sprite_capacity(&t).is_err());
    }

    #[test]
    fn starburst_waits_delay_and_emits_thirteen_retail_rays() {
        let t = authored_sprite(1006);
        let mut a = active_sprite(&t);
        let mut rng = CrtRand::new(1);
        let mut random = R250::new(0xe6f1);
        emit_sprites(&t,&mut a,&mut rng,&mut random,16,0.0);
        assert!(a.particles.is_empty());
        a.elapsed = 0.01;
        emit_sprites(&t,&mut a,&mut rng,&mut random,16,0.01);
        assert_eq!(a.particles.len(),13);
        assert_eq!(a.repetitions,0);
        assert!(a.particles.iter().all(|p| p.velocity_p.is_finite() && p.velocity_q.is_finite()));
        a.particles.clear();
        a.elapsed = 1.0;
        emit_sprites(&t,&mut a,&mut rng,&mut random,16,0.01);
        a.elapsed = 1.01;
        emit_sprites(&t,&mut a,&mut rng,&mut random,16,0.01);
        assert!(a.particles.is_empty());
    }

    #[test]
    fn persistent_buff_renderer_remove_refresh_and_death() {
        let dir=ao_gui::client_dir();
        if !dir.join("Setupf/gfxtweak.bin").exists() || !dir.join("cd_image/rdb.db").exists() {return;}
        let mut renderer=Renderer::open(&dir).unwrap();
        let identity=(50000,1234);
        let binding=Binding {group:0,attractor:0,effect:11506,note:0,color:0};
        let config=EffectConfig {source_identity:Some(identity),..Default::default()};
        let first=renderer.spawn_configured(binding,Mat4::IDENTITY,Vec3::ZERO,config).unwrap();
        assert!(renderer.is_active(first));
        assert!(renderer.needs_source_mesh(identity));
        renderer.prepare_source_mesh(identity,&Scene::default(),&ActorFrame {id:1234,..Default::default()});
        assert!(!renderer.needs_source_pose(identity));
        renderer.terminate_gracefully(first);
        assert!(renderer.is_active(first),"native mode2 keeps its ramp-down tail");
        renderer.delete(first);
        assert!(!renderer.is_active(first));
        let refreshed=renderer.spawn_configured(binding,Mat4::IDENTITY,Vec3::ZERO,config).unwrap();
        assert_ne!(refreshed,first);
        assert!(renderer.is_active(refreshed));
        renderer.clear();
        assert!(!renderer.is_active(refreshed));
        assert!(!renderer.needs_source_mesh(identity));
        assert!(renderer.buff_models.is_empty());
    }

    #[test]
    fn deleted_source_kills_shield_but_missing_pose_preserves_it() {
        let dir=ao_gui::client_dir();
        if !dir.join("Setupf/gfxtweak.bin").exists() || !dir.join("cd_image/rdb.db").exists() {return;}
        let mut renderer=Renderer::open(&dir).unwrap();
        let identity=(50000,1234);
        let binding=Binding {group:0,attractor:0,effect:72260,note:0,color:0};
        let handle=renderer.spawn_configured(binding,Mat4::IDENTITY,Vec3::ZERO,EffectConfig {source_identity:Some(identity),..Default::default()}).unwrap();
        assert!(renderer.needs_source_pose(identity));
        renderer.prepare_source_mesh(identity,&Scene::default(),&ActorFrame {id:1234,..Default::default()});
        assert!(renderer.is_active(handle),"missing fresh posed geometry is not deletion");
        renderer.terminate_gracefully(handle);
        assert!(renderer.is_active(handle),"native Shield2 retains its two-second drain");
        renderer.source_deleted(identity);
        assert!(!renderer.is_active(handle),"actual source destruction bypasses the drain");
        assert!(!renderer.buff_models.contains_key(&handle));
        let trail=renderer.spawn_configured(Binding {effect:72422,..binding},Mat4::IDENTITY,Vec3::ZERO,EffectConfig {source_identity:Some(identity),..Default::default()}).unwrap();
        renderer.source_deleted(identity);
        assert!(!renderer.is_active(trail),"native Trail2 deletes with its source dynel");
    }

    #[test]
    fn buff_refresh_retires_gpu_models_and_same_key_appearance_stops_surface() {
        let dir=ao_gui::client_dir();
        if !dir.join("Setupf/gfxtweak.bin").exists() || !dir.join("cd_image/rdb.db").exists() {return;}
        let mut renderer=Renderer::open(&dir).unwrap();
        // Finite authored duration exercises expiry as well as explicit delete/clear.
        renderer.templates.by_id.get_mut(&72260).unwrap().words[8]=4.0f32.to_bits();
        let identity=(50000,1234);
        let config=EffectConfig {source_identity:Some(identity),..Default::default()};
        let binding=Binding {group:0,attractor:0,effect:72260,note:0,color:0};
        let vertices=vec![Vertex {pos:[0.0,0.0,0.0],..Default::default()};3];
        let scene=Scene {meshes:vec![Mesh {vertices:vertices.clone(),submeshes:vec![Submesh::new(vec![0,1,2],None)]}],..Default::default()};
        let actor=ActorFrame {id:1234,model:super::super::super::avatar::MODEL_KEY,transform:IDENTITY,skin:Some(vertices),alpha:1.0,..Default::default()};
        let mut host=ao_render::Host::headless();
        let mut uploaded=std::collections::HashSet::new();
        for cycle in 0..32 {
            let handle=renderer.spawn_configured(binding,Mat4::IDENTITY,Vec3::ZERO,config).unwrap();
            renderer.prepare_source_mesh(identity,&scene,&actor);
            renderer.frame(0.01,&mut host,None);
            for key in host.actor_model_removals.drain(..) {uploaded.remove(&key);}
            for (key,_) in host.actor_models.drain(..) {uploaded.insert(key);}
            assert_eq!(uploaded.len(),1,"refresh must not retain previous per-handle GPU models");
            assert!(host.actors.iter().any(|a|a.id==handle));
            host.actors.clear();
            renderer.source_model_changed(identity);
            assert!(renderer.needs_source_pose(identity));
            let mut replacement=scene.clone();
            replacement.meshes[0].submeshes[0].indices=vec![2,1,0];
            renderer.prepare_source_mesh(identity,&replacement,&actor);
            renderer.frame(0.01,&mut host,None);
            assert!(host.actors.is_empty(),"native CAT replacement Stop applies even with unchanged model key and vertex count");
            match cycle%3 {
                0=>renderer.delete(handle),
                1=>renderer.clear(),
                _=>{renderer.frame(1000.0,&mut host,None);renderer.frame(3.0,&mut host,None);},
            }
            renderer.frame(0.01,&mut host,None);
            for key in host.actor_model_removals.drain(..) {uploaded.remove(&key);}
            for (key,_) in host.actor_models.drain(..) {uploaded.insert(key);}
            assert!(uploaded.is_empty(),"deleted effect GPU model must be retired");
            host.actors.clear();
        }
    }

    #[test]
    #[ignore = "retail assets and offscreen Metal rendering"]
    fn retail_effect_frames() {
        let dir = ao_gui::client_dir();
        let out = std::env::var_os("AOMAC_EFFECT_FRAMES").map(std::path::PathBuf::from).unwrap_or_else(|| "/tmp/FxRest/frames".into());
        std::fs::create_dir_all(&out).unwrap();
        let mut renderer = Renderer::open(&dir).unwrap();
        let continuous = renderer.templates.by_id.iter().filter(|(_,t)| t.kind==1005 && t.float(10).is_ok_and(|r| r>0.0)).map(|(&id,_)| id).min().expect("retail continuous flare");
        let origin = Vec3::new(5000.0,10.0,5000.0);
        let eye = origin+Vec3::new(0.4,0.3,2.0);
        let mut host = ao_render::Host::headless();
        host.camera = ao_render::Camera::look_at(eye,origin);
        for id in [2000,2601,continuous,2750] {
            renderer.clear();
            renderer.spawn(Binding { group:0,attractor:0,effect:id,note:0,color:0 },Mat4::from_translation(origin),origin+Vec3::X*4.0).unwrap();
            let mut previous = 0.0;
            for time in [0.01,0.04,0.08,0.15,0.4] {
                host.actors.clear();
                renderer.frame(time-previous,&mut host,None);
                previous = time;
                assert!(host.actors.iter().all(|a| a.always && a.skin.as_ref().unwrap().iter().all(|v| v.pos.iter().all(|f| f.is_finite()))));
                let models: Vec<_> = renderer.models.iter().map(|(&id,m)| (MODEL_BASE|id as u32 as u64,m.scene.clone())).collect();
                ao_render::render_to_png_actors(&Scene::default(),&models,host.actors.clone(),eye.to_array(),origin.to_array(),640,480,&out.join(format!("{id}_{time:.2}.png")),time).unwrap();
            }
        }
    }


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
        for a in &mut renderer.active {
            let t = &renderer.templates.by_id[&a.effect];
            if t.kind == 1005 { emit_sprites(t,a,&mut renderer.rng,&mut renderer.random,sprite_capacity(t).unwrap(),0.0); }
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
