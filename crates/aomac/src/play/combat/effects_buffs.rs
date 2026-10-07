//! Native persistent-buff classes. Controllers own real child handles; surface
//! controls receive the live posed CAT surface, never replacement geometry.
use super::{buff200x, buff300x, buff303x, buff_shield, buffelectra, buffstars, crystal, highlight, EffectConfig, Renderer, Template};
use anyhow::{ensure, Result};
use ao_formats::{character::CrtRand, weather::R250};
use ao_scene::{ActorFrame, Blend, Scene, TextureKey, Vertex};
use glam::{Mat4, Vec3};

// Effect storage owns Box<Buff>; boxing variants would add a second allocation
// to the same controller without reducing the renderer's per-effect layout.
#[allow(clippy::large_enum_variant)]
enum Native {
    Stars(buffstars::StarsEffect), Suns(buff200x::Suns), Electra(buffelectra::ElectraEffect),
    Highlight(highlight::Highlight), Shield(buff_shield::Shield), Sequencer(buff300x::Sequencer),
    SkyFlash(buff300x::SkyFlash), Scatter(buff303x::ScatterEffect),
    Shield2 {effect:buff303x::ShieldEffect, vertices:Vec<Vertex>, indices:Vec<u32>, transform:Mat4},
    Grid(buff303x::GridEffect), Trail(buff303x::TrailEffect),
    Crystal(crystal::Crystal),
}
pub(super) struct Buff {
    native: Native,
    config: EffectConfig,
    source: Mat4,
    target: Mat4,
    source_ready: bool,
    source_model: Option<u64>,
    source_texture: Option<TextureKey>,
}
impl Buff {
    pub(super) fn supports(kind:i32)->bool {matches!(kind,2004|2005|2006|2011|3003|3004|3006|3029|3031|3034|3038|3039)}
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(t:&Template,source:Mat4,target:Vec3,color:u32,c:EffectConfig,gc:&mut R250,ds:&mut R250,crt:&mut CrtRand)->Result<Self> {
        let target=Mat4::from_translation(target);
        let native=match t.kind {
            2004=>{let mut n=buffstars::StarsEffect::new(t,source,target,color,gc,ds,crt)?;n.configure(c)?;Native::Stars(n)},
            2005=>{let mut n=buff200x::Suns::new(t,source,target,color,gc,ds,crt)?;n.configure(c)?;Native::Suns(n)},
            2006=>{let mut n=buffelectra::ElectraEffect::new(t,source,target,color,gc,ds,crt)?;n.configure(c)?;Native::Electra(n)},
            2011=>Native::Highlight(highlight::Highlight::new(t,c)?),
            3003=>Native::Shield(buff_shield::Shield::new(t,source,c)?),
            3004=>Native::Sequencer(buff300x::Sequencer::new(t,source,target.w_axis.truncate(),c)?),
            3006=>{let mut n=buff300x::SkyFlash::new(t,source)?;n.configure(c);Native::SkyFlash(n)},
            3029=>Native::Scatter(buff303x::ScatterEffect::new(t,source,target.w_axis.truncate(),c,gc)?),
            3031=>{let mut n=crystal::Crystal::new(t,source,gc)?;n.configure(c);Native::Crystal(n)},
            3034=>Native::Shield2 {effect:buff303x::ShieldEffect::new(t,c)?,vertices:Vec::new(),indices:Vec::new(),transform:source},
            3038=>{let mut n=buff303x::GridEffect::new(t,source,target,color,gc,ds,crt)?;n.configure(c)?;Native::Grid(n)},
            3039=>{let mut n=buff303x::TrailEffect::new(t,source,target,color,gc,ds,crt)?;n.configure(c)?;Native::Trail(n)},
            _=>anyhow::bail!("unsupported native buff class {}",t.kind),
        };
        Ok(Self {native,config:c,source,target,source_ready:false,source_model:None,source_texture:None})
    }
    pub(super) fn identity(&self)->Option<(u32,u32)> {match &self.native {Native::Shield(n)=>Some(n.identity()),Native::Highlight(n)=>Some(n.identity()),_=>self.config.source_identity}}
    pub(super) fn needs_source_mesh(&self)->bool {match &self.native {Native::Shield(_)|Native::Shield2 {..}|Native::Highlight(_)=>true,Native::Stars(n)=>n.needs_source_geometry(),_=>false}}
    pub(super) fn needs_source_pose(&self)->bool {self.needs_source_mesh() && !self.source_ready}
    pub(super) fn source_texture(&self)->Option<TextureKey> {match &self.native {Native::Shield(n) if n.uses_source_material()=>self.source_texture,_=>None}}
    pub(super) fn needs_private_model(&self)->bool {matches!(self.native,Native::Shield(_)|Native::Shield2 {..})}
    pub(super) fn source_model_changed(&mut self) {
        if !self.needs_source_mesh() {return;}
        if self.source_model.is_some() {
            if let Native::Shield2 {effect,..}=&mut self.native {effect.stop_surface();}
        }
        self.source_model=None;
        self.source_ready=false;
    }
    pub(super) fn prepare_source(&mut self,scene:&Scene,actor:&ActorFrame,crt:&mut CrtRand)->Result<bool> {
        if let Native::Highlight(n)=&mut self.native {n.prepare_actor(actor.id);self.source_ready=true;return Ok(false);}
        let Some(vertices)=actor.skin.as_deref() else {return Ok(false)};
        let Some(mesh)=scene.meshes.first() else {return Ok(false)};
        ensure!(vertices.len()==mesh.vertices.len(),"posed buff surface does not match CAT model");
        let changed=self.source_model!=Some(actor.model);
        let indices:Vec<_>=if changed {mesh.submeshes.iter().flat_map(|s|s.indices.iter().copied()).collect()}else{Vec::new()};
        self.source_texture=mesh.submeshes.first().and_then(|s|s.texture);
        match &mut self.native {
            Native::Stars(n)=>{n.set_source_root(Mat4::from_cols_array_2d(&actor.transform));n.set_source_geometry(vertices,&indices,crt)?;},
            Native::Shield(n)=>{
                if changed {n.update_mesh(vertices,&indices,None)?;}else{n.update_vertices(vertices)?;}
                n.update_source(Mat4::from_cols_array_2d(&actor.transform));
            },
            Native::Shield2 {effect,vertices:body,indices:body_indices,transform}=>{
                if changed && self.source_model.is_some() {effect.stop_surface();}
                body.clear();body.extend_from_slice(vertices);
                if changed {*body_indices=indices;}
                *transform=Mat4::from_cols_array_2d(&actor.transform);
            },
            _=>return Ok(false),
        }
        self.source_model=Some(actor.model);self.source_ready=true;
        Ok(changed)
    }
    pub(super) fn update_source(&mut self,source:Mat4)->Result<()> {
        self.source=source;
        if let Native::Crystal(n)=&mut self.native {return n.update_source(source);}
        match &mut self.native {
            Native::Stars(n)=>n.update_source(source)?,Native::Suns(n)=>n.update_source(source)?,Native::Electra(n)=>n.update_source(source)?,
            Native::Shield(n)=>n.update_source(source),Native::Sequencer(n)=>n.update_source(source),Native::SkyFlash(n)=>n.update_source(source)?,
            Native::Scatter(n)=>n.update_source(source),Native::Grid(n)=>n.update_source(source)?,Native::Trail(n)=>n.update_source(source)?,_=>{},
        } Ok(())
    }
    pub(super) fn refresh_anchors(&mut self,mut get:impl FnMut(i32)->Option<Mat4>)->Result<()> {
        match &mut self.native {
            Native::Stars(n)=>{n.update_anchors(self.source,self.target)?;for i in 0..n.required_attractors().len() {let id=n.required_attractors()[i];if let Some(m)=get(id) {n.set_attractor(id,m.w_axis.truncate());}}},
            Native::Suns(n)=>{for i in 0..n.required_attractors().len() {let id=n.required_attractors()[i];n.set_attractor(id,get(id));}n.update_hit(self.source.w_axis.truncate(),self.target.w_axis.truncate());},
            Native::Electra(n)=>{for i in 0..n.anchor_ids().len() {let id=n.anchor_ids()[i];n.update_anchor(id,get(id));}},
            _=>{},
        } Ok(())
    }
    pub(super) fn frame(&mut self,dt:f32,renderer:&mut Renderer)->Result<bool> {
        Ok(match &mut self.native {
            Native::Highlight(n)=>n.advance(dt),Native::Shield(n)=>n.frame(dt)?,Native::Sequencer(n)=>n.frame(dt,renderer)?,
            Native::SkyFlash(n)=>n.frame(dt)?,Native::Scatter(n)=>n.frame(dt,renderer)?,Native::Shield2 {effect,..}=>effect.frame(dt),_=>true,
        })
    }
    pub(super) fn graceful(&mut self,renderer:&mut Renderer)->Result<()> {
        match &mut self.native {
            Native::Crystal(_)=>unreachable!("class3031 termination deletes centrally"),
            Native::Stars(n)=>n.terminate_gracefully(),Native::Suns(n)=>n.terminate_gracefully(),Native::Electra(n)=>n.terminate_gracefully(),
            Native::Highlight(n)=>n.terminate_gracefully(),Native::Shield(n)=>n.terminate_gracefully(),Native::Sequencer(n)=>n.terminate_gracefully(renderer),
            Native::SkyFlash(n)=>n.terminate_gracefully(),Native::Scatter(n)=>n.graceful(renderer)?,Native::Shield2 {effect,..}=>effect.graceful(),
            Native::Grid(_)=>{},Native::Trail(n)=>n.graceful(),
        } Ok(())
    }
    pub(super) fn cancel(&mut self,renderer:&mut Renderer)->Result<()> {match &mut self.native {Native::Sequencer(n)=>n.cancel(renderer),Native::Scatter(n)=>n.cancel(renderer)?,_=>{}}Ok(())}
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {
        match &self.native {
            Native::Stars(n)=>n.models(),Native::Suns(n)=>n.models(),Native::Electra(n)=>n.models(),Native::Shield(n)=>n.models(),
            Native::Crystal(n)=>n.models(),
            Native::SkyFlash(n)=>n.models(),Native::Grid(n)=>n.models(),Native::Trail(n)=>n.models(),
            Native::Shield2 {effect,vertices,indices,..}=>{let n=vertices.len();let out=(0..effect.layer_count()).flat_map(|layer|indices.iter().map(move |&i|i+(layer*n) as u32)).collect();vec![(Some(effect.material()),out,n*effect.layer_count())]},
            _=>Vec::new(),
        }
    }
    pub(super) fn blends(&self)->Vec<Blend> {match &self.native {
        Native::Crystal(n)=>n.blends(),
        Native::Stars(n)=>n.blends(),Native::Suns(n)=>n.blends(),Native::Electra(n)=>n.blends(),Native::Shield(n)=>n.blends(),Native::SkyFlash(n)=>n.blends(),Native::Grid(n)=>n.blends(),Native::Trail(n)=>n.blends(),Native::Shield2 {effect,..}=>vec![effect.blend()],_=>Vec::new(),
    }}
    pub(super) fn apply_material(&self,actors:&mut[ActorFrame]) {if let Native::Highlight(n)=&self.native {n.apply(actors);}}
    #[allow(clippy::too_many_arguments)]
    pub(super) fn vertices(&mut self,time:f32,camera:Vec3,right:Vec3,up:Vec3,gc:&mut R250,ds:&mut R250,crt:&mut CrtRand,terrain:Option<&mut dyn FnMut(Vec3)->Option<(Vec3,Vec3)>>)->Result<Option<Vec<Vec<Vertex>>>> {
        if self.needs_source_pose() {return Ok(Some(Vec::new()));}
        match &mut self.native {
            Native::Crystal(n)=>n.vertices(time,gc),
            Native::Stars(n)=>{if let Some(terrain)=terrain {if let Some((p,_))=terrain(self.source.w_axis.truncate()) {n.set_ground_height(p.y);}}n.vertices(time,camera,right,up,gc,ds,crt)},
            Native::Suns(n)=>n.vertices(time,camera,right,up,gc,ds,crt),Native::Electra(n)=>n.vertices(time,camera,right,up,gc,ds,crt),
            Native::Shield(n)=>n.vertices(),Native::SkyFlash(n)=>{
                if let Some(terrain)=terrain {n.vertices(terrain)} else {
                    ensure!(!n.requires_terrain(),"SkyFlash requires actual terrain locator");
                    n.vertices(&mut |_|None)
                }
            },
            Native::Grid(n)=>n.vertices(time,camera,right,up,gc,ds,crt),Native::Trail(n)=>n.vertices(time,camera,right,up,gc,ds,crt),
            Native::Shield2 {effect,vertices,transform,..}=>{let mut out=effect.vertices(vertices,1.0);for v in &mut out {v.pos=transform.transform_point3(Vec3::from_array(v.pos)).to_array();}Ok(Some(vec![out]))},
            _=>Ok(Some(Vec::new())),
        }
    }
}
