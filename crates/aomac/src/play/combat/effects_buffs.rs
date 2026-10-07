//! Native persistent-buff classes. Controllers own real child handles; surface
//! controls receive the live posed CAT surface, never replacement geometry.
use super::{buff200x, buff300x, buff303x, buff_shield, buffelectra, buffstars, crystal, groundgrid, highlight, lavaball, legacy_buffs, legacy_sprites, legacy300x, legacy301x, legacy302x, legacy303x, native20012, native30001, native301722, vein, EffectConfig, Renderer, Template};
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
    GroundGrid(groundgrid::GroundGrid),
    Spiral(native20012::Spiral), Plasma(native20012::Plasma),
    ShockWave(native30001::ShockWave),
    Deformer {effect:native30001::Deformer, visual:Option<native30001::MorphVisual>, original:Vec<Vertex>, child:Option<u32>},
    Native301722(native301722::NativeEffect),
    Spiral2(legacy303x::Spiral2), AParticle(legacy303x::AParticle), Toggle(legacy303x::Toggle),
    SkyRise(legacy302x::SkyRise), EnergyBall(legacy302x::EnergyBall), LegacyTrail(legacy302x::Trail),
    Splash(legacy300x::Splash), WaterRipples(legacy300x::WaterRipples), CrazyCone(legacy300x::CrazyCone),
    LavaBall(lavaball::LavaBallEffect),
    GroundRing(legacy301x::GroundRing), Bubble(legacy301x::Bubble), Delay(legacy301x::Delay),
    Shadow {effect:legacy300x::Shadow,indices:Vec<u32>,count:usize},
    Vein(vein::Vein),
    Font(legacy_buffs::Font), Fence(legacy_buffs::Fence), Notum(legacy_buffs::Notum),
    Hexagram(legacy_sprites::Hexagram), Drips(legacy_sprites::Drips),
}
pub(super) struct Buff {
    native: Native,
    config: EffectConfig,
    source: Mat4,
    target: Mat4,
    source_ready: bool,
    source_model: Option<u64>,
    source_texture: Option<TextureKey>,
    frame_dt:f32,
    clock:Option<i32>,
    source_valid:bool,
    texture_clamp:bool,
}
impl Buff {
    pub(super) fn supports(kind:i32)->bool {matches!(kind,2001|2002|2004|2005|2006|2008|2009|2011|2012|2014|2015|3000|3001|3002|3003|3004|3005|3006|3008|3009|3012|3013|3014|3015|3016|3017|3018|3019|3022|3023|3029|3030|3031|3033|3034|3035|3036|3038|3039)}
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(t:&Template,source:Mat4,target:Vec3,color:u32,c:EffectConfig,gc:&mut R250,ds:&mut R250,crt:&mut CrtRand,terrain:Option<&mut dyn FnMut(Vec3)->Option<(Vec3,Vec3)>>,resources:Option<(&ao_rdb::RecordStore,u32)>)->Result<Self> {
        let target=Mat4::from_translation(target);
        let native=match t.kind {
            2001=>Native::Spiral(native20012::Spiral::new(t,source,c)?),
            2002=>Native::Plasma(native20012::Plasma::new(t,c.hit_location,c)?),
            3000=>{let mut n=native30001::ShockWave::new(t,source)?;n.configure(c);Native::ShockWave(n)},
            3001=>Native::Deformer {effect:native30001::Deformer::new(t,c)?,visual:None,original:Vec::new(),child:None},
            3030=>Native::GroundGrid(groundgrid::GroundGrid::new(t,source,c,gc)?),
            3017|3022=>Native::Native301722(native301722::NativeEffect::new(t,source,c,gc,ds)?),
            3033=>Native::Spiral2(legacy303x::Spiral2::new(t,source,c,gc)?),
            3035=>Native::AParticle(legacy303x::AParticle::new(t,source,c,gc)?),
            3036=>Native::Toggle(legacy303x::Toggle::new(t,source,target.w_axis.truncate(),c)?),
            3018=>{let source=super::sprites::connector(t,source)?;let terrain=terrain.ok_or_else(||anyhow::anyhow!("SkyRise requires actual terrain locator"))?;let mut n=legacy302x::SkyRise::new(t,source,terrain)?;n.configure(c);Native::SkyRise(n)},
            3019=>{let mut n=legacy302x::Trail::new(t,super::sprites::connector(t,source)?)?;n.configure(c);Native::LegacyTrail(n)},
            3023=>{let source=super::sprites::connector(t,source)?;let terrain=terrain.ok_or_else(||anyhow::anyhow!("EnergyBall requires actual terrain locator"))?;let mut n=legacy302x::EnergyBall::new(t,source,gc,terrain)?;n.configure(c);Native::EnergyBall(n)},
            3002=>{let mut n=legacy300x::Splash::new(t,source)?;n.configure(c);Native::Splash(n)},
            3005=>{let mut n=legacy300x::WaterRipples::new(t,source)?;n.configure(c);Native::WaterRipples(n)},
            3009=>{let mut n=legacy300x::CrazyCone::new(t,source)?;n.configure(c);Native::CrazyCone(n)},
            3015=>Native::LavaBall(lavaball::LavaBallEffect::new(t,source,if c.creation==super::Creation::Dynel {Vec3::ZERO}else{source.w_axis.truncate()},c)?),
            3012=>{let mut n=legacy301x::GroundRing::new(t,source)?;n.configure(c);Native::GroundRing(n)},
            3016=>{let mut n=legacy301x::Bubble::new(t,source)?;n.configure(c);if c.source_nonvisual {n.invalidate_source();}Native::Bubble(n)},
            3013=>Native::Delay(legacy301x::Delay::new(t,source,c,crt)?),
            3008=>{let mut effect=legacy300x::Shadow::new(t,source)?;effect.configure(c);Native::Shadow {effect,indices:Vec::new(),count:0}},
            3014=>Native::Vein(vein::Vein::new(t,source,c)?),
            2014=>Native::Font(legacy_buffs::Font::new(t,source,c)?),
            2012=>{let (store,pfid)=resources.ok_or_else(||anyhow::anyhow!("Fence requires actual playfield boundary resource"))?;Native::Fence(legacy_buffs::Fence::new(t,source,store,pfid)?)},
            2015=>Native::Notum(legacy_buffs::Notum::new(t,source,c.creation)?),
            2008=>{
                let terrain=terrain.ok_or_else(||anyhow::anyhow!("Hexagram requires actual terrain locator"))?;
                // GC100d2e9e initializes dungeon query height to zero even on a miss.
                let mut n=legacy_sprites::Hexagram::new(t,source,|p|terrain(p).map_or(0.0,|(p,_)|p.y))?;
                n.configure(c);Native::Hexagram(n)
            },
            2009=>{let mut n=legacy_sprites::Drips::new(t,source)?;n.configure(c);Native::Drips(n)},
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
        Ok(Self {native,config:c,source,target,source_ready:false,source_model:None,source_texture:None,frame_dt:0.0,clock:None,source_valid:!c.source_nonvisual,texture_clamp:t.kind==3030 && t.word(0)?&0x10000!=0})
    }
    pub(super) fn identity(&self)->Option<(u32,u32)> {match &self.native {Native::Shield(n)=>Some(n.identity()),Native::Highlight(n)=>Some(n.identity()),_=>self.config.source_identity}}
    pub(super) fn texture_clamp(&self)->bool {self.texture_clamp}
    pub(super) fn needs_source_mesh(&self)->bool {match &self.native {Native::Shield(_)|Native::Highlight(_)|Native::Deformer {..}|Native::Shadow {..}=>true,Native::Shield2 {effect,..}=>effect.mech_resource().is_none(),Native::Stars(n)=>n.needs_source_geometry(),_=>false}}
    pub(super) fn needs_source_pose(&self)->bool {match &self.native {
        Native::Shield(_)|Native::Shadow {..}=>true,
        Native::Shield2 {effect,..}=>effect.mech_resource().is_none() && !effect.is_surface_stopped(),
        Native::Deformer {effect,..}=>effect.mode()!=4,
        Native::Stars(n)=>n.needs_source_geometry(),
        _=>self.needs_source_mesh() && !self.source_ready,
    }}
    pub(super) fn source_texture(&self)->Option<TextureKey> {match &self.native {Native::Shield(n) if n.uses_source_material()=>self.source_texture,Native::Shield2 {effect,..} if effect.uses_source_material()=>self.source_texture,_=>None}}
    pub(super) fn needs_private_model(&self)->bool {matches!(self.native,Native::Shield(_)|Native::Shield2 {..}|Native::WaterRipples(_)|Native::Shadow {..}|Native::Font(_))}
    pub(super) fn priority(&self)->Option<i32> {match &self.native {Native::Shield2 {effect,..}=>effect.priority(),Native::LavaBall(n)=>Some(n.priority()),Native::Native301722(_)|Native::Spiral2(_)|Native::AParticle(_)=>Some(6),Native::Shadow {..}=>Some(2),_=>None}}
    pub(super) fn morph_child(&self)->Option<(i32,f32)> {match &self.native {Native::Deformer {effect,..} if effect.mode()==4=>Some((11762,effect.transition_time())),_=>None}}
    pub(super) fn set_morph_child(&mut self,handle:u32) {if let Native::Deformer {child,..}=&mut self.native {*child=(handle!=0).then_some(handle);}}
    pub(super) fn source_model_changed(&mut self) {
        if !self.needs_source_mesh() {return;}
        if self.source_model.is_some() {
            if let Native::Shield2 {effect,..}=&mut self.native {effect.stop_surface();self.source_ready=true;return;}
        }
        self.source_model=None;
        self.source_ready=false;
    }
    pub(super) fn prepare_source(&mut self,scene:&Scene,actor:&ActorFrame,crt:&mut CrtRand)->Result<bool> {
        if let Native::Highlight(n)=&mut self.native {n.prepare_actor(actor.id);self.source_ready=true;return Ok(false);}
        if let Native::Deformer {effect,original,..}=&mut self.native {
            effect.prepare_source(scene,actor)?;
            if let Some(mesh)=scene.meshes.first() {original.clone_from(&mesh.vertices);}
            self.source_ready=true;
            return Ok(false);
        }
        if let Native::Shadow {effect,..}=&mut self.native {effect.prepare_source(scene,actor)?;self.source_ready=true;return Ok(false);}
        if let Native::Shield2 {effect,..}=&self.native {if effect.is_surface_stopped() {self.source_ready=true;return Ok(false);}}
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
                if effect.is_surface_stopped() {self.source_ready=true;return Ok(false);}
                body.clear();body.extend_from_slice(vertices);
                if changed {*body_indices=indices;}
                *transform=Mat4::from_cols_array_2d(&actor.transform);
            },
            _=>return Ok(false),
        }
        self.source_model=Some(actor.model);self.source_ready=true;
        Ok(changed)
    }
    pub(super) fn mech_resource(&self)->Option<&'static str> {match &self.native {Native::Shield2 {effect,..}=>effect.mech_resource(),_=>None}}
    pub(super) fn prepare_mech(&mut self,scene:&Scene)->Result<()> {
        if let Native::Shield2 {vertices,indices,..}=&mut self.native {
            vertices.clear();indices.clear();
            let mut append=|mesh:&ao_scene::Mesh,transform:Mat4|->Result<()> {
                let base=u32::try_from(vertices.len())?;
                let normal=transform.inverse().transpose();
                vertices.extend(mesh.vertices.iter().map(|v|Vertex {pos:transform.transform_point3(Vec3::from_array(v.pos)).to_array(),normal:normal.transform_vector3(Vec3::from_array(v.normal)).normalize_or_zero().to_array(),..*v}));
                for sub in &mesh.submeshes {indices.extend(sub.indices.iter().map(|&i|i+base));}
                Ok(())
            };
            if scene.instances.is_empty() {for mesh in &scene.meshes {append(mesh,Mat4::IDENTITY)?;}}
            else {for instance in &scene.instances {let mesh=scene.meshes.get(instance.mesh).ok_or_else(||anyhow::anyhow!("invalid Shield2 mech instance"))?;append(mesh,Mat4::from_cols_array_2d(&instance.transform))?;}}
            ensure!(!vertices.is_empty(),"Shield2 mech resource has no surface");
            self.source_ready=true;
        }
        Ok(())
    }
    pub(super) fn prepare_morph(&mut self,store:&ao_rdb::RecordStore)->Result<()> {
        if let Native::Deformer {effect,visual,..}=&mut self.native {
            if effect.mode()==4 && visual.is_none() {*visual=Some(native30001::MorphVisual::load(store)?);}
        }
        Ok(())
    }
    pub(super) fn morph_scene(&self)->Option<&Scene> {match &self.native {Native::Deformer {visual:Some(v),..}=>Some(v.model()),_=>None}}
    pub(super) fn morph_actor(&self,id:u32,model:u64)->Option<ActorFrame> {
        let Native::Deformer {effect,visual:Some(visual),..}=&self.native else {return None};
        let (vertices,parts)=visual.pose(effect);
        Some(ActorFrame {id,model,transform:self.source.to_cols_array_2d(),skin:Some(vertices),parts,alpha:effect.morph_alpha(),always:true,..Default::default()})
    }
    pub(super) fn set_environment_position(&mut self,position:Vec3) {if let Native::Native301722(n)=&mut self.native {n.set_environment_position(position);}}
    pub(super) fn set_visible(&mut self,visible:bool) {match &mut self.native {Native::Spiral(n)=>n.set_visible(visible),Native::Native301722(n)=>n.set_source_visible(visible),_=>{}}}
    pub(super) fn source_removed(&mut self) {self.source_valid=false;match &mut self.native {Native::Spiral(n)=>n.source_removed(),Native::Spiral2(n)=>n.source_removed(),Native::AParticle(n)=>n.source_removed(),Native::Toggle(n)=>n.source_removed(),Native::GroundRing(n)=>n.invalidate_source(),Native::Bubble(n)=>n.invalidate_source(),Native::LavaBall(n)=>n.source_lost(),_=>{}}}
    pub(super) fn refresh_source(&mut self,source:Option<Mat4>)->Result<()> {self.source_valid=source.is_some();if let Native::Vein(n)=&mut self.native {n.refresh_source(source)?;}Ok(())}
    pub(super) fn set_environment(&mut self,center:Vec3,wind:Vec3) {if let Native::AParticle(n)=&mut self.native {n.set_environment(center,wind);}}
    pub(super) fn set_playfield(&mut self,id:u32,flags:u32) {if let Native::Toggle(n)=&mut self.native {n.set_playfield(id,flags);}}
    pub(super) fn set_motion(&mut self,speed:f32,direction:i32) {if let Native::Toggle(n)=&mut self.native {n.set_motion(speed,direction);}}
    pub(super) fn set_torso_factor(&mut self,scale:f32) {if let Native::Spiral2(n)=&mut self.native {n.set_torso_factor(scale);}}
    pub(super) fn set_vehicle_direction(&mut self,direction:Vec3) {if let Native::Trail(n)=&mut self.native {n.set_vehicle_direction(direction);}}
    pub(super) fn update_hit_location(&mut self,hit:Option<(Vec3,Vec3)>)->Result<()> {if let Native::Plasma(n)=&mut self.native {n.update_hit_location(hit)?;}Ok(())}
    pub(super) fn update_liquid(&mut self,depth:f32,flags:u32,direction:Vec3) {if let Native::WaterRipples(n)=&mut self.native {n.update_liquid(depth,flags,direction);}}
    pub(super) fn clear_liquid(&mut self) {if let Native::WaterRipples(n)=&mut self.native {n.clear_liquid();}}
    pub(super) fn take_sounds(&mut self)->Vec<super::AuxSound> {match &mut self.native {Native::LavaBall(n)=>n.take_sounds(),Native::Notum(n)=>n.take_sounds().into_iter().map(|(name,p)|super::AuxSound {id:ao_audio::sbf::sound_id(name),pos:p.to_array(),velocity:[0.0;3],parameters:[1.0,0.0,0.0,0.0],probability:100}).collect(),_=>Vec::new()}}
    pub(super) fn render_bucket(&self)->Option<i32> {match &self.native {Native::LavaBall(n)=>n.render_bucket(),_=>None}}
    pub(super) fn dynamic_models(&self)->bool {matches!(self.native,Native::WaterRipples(_)|Native::Shadow {..})}
    pub(super) fn forwarded_child(&self)->Option<u32> {match &self.native {Native::Delay(n) if n.child()!=0=>Some(n.child()),_=>None}}
    pub(super) fn initialize(&mut self,renderer:&Renderer)->Result<()> {if let Native::Delay(n)=&mut self.native {n.initialize(renderer,self.config)?;}Ok(())}
    pub(super) fn set_text(&mut self,text:&[u8])->Result<()> {if let Native::Font(n)=&mut self.native {n.set_text(text)?;}Ok(())}
    pub(super) fn set_clock(&mut self,seconds:i32) {self.clock=Some(seconds);}
    pub(super) fn set_shadow_enabled(&mut self,enabled:bool) {if let Native::Shadow {effect,..}=&mut self.native {effect.set_enabled(enabled);}}
    #[allow(clippy::too_many_arguments)]
    pub(super) fn update_shadow_world(&mut self,body:Vec3,head_height:f32,sun:Vec3,camera:Vec3,dungeon:bool,ground:&mut dyn FnMut(Vec3)->Option<(Vec3,Vec3)>,ray:&mut dyn FnMut(Vec3,Vec3)->Option<(Vec3,Vec3)>)->Result<()> {if let Native::Shadow {effect,..}=&mut self.native {effect.update_world((body,head_height),sun,camera,dungeon,ground,ray)?;}Ok(())}
    pub(super) fn update_source(&mut self,source:Mat4)->Result<()> {
        self.source=source;
        if let Native::Crystal(n)=&mut self.native {return n.update_source(source);}
        match &mut self.native {
            Native::Font(n)=>n.update_source(source)?,Native::Fence(n)=>n.update_source(source)?,
            Native::Hexagram(n)=>n.update_source(source)?,Native::Drips(n)=>n.update_source(source)?,
            Native::GroundRing(n)=>n.update_source(source)?,Native::Bubble(n)=>n.update_source(source)?,Native::Shadow {effect,..}=>effect.update_source(source)?,
            Native::Splash(n)=>n.update_source(source)?,Native::WaterRipples(n)=>n.update_source(source)?,Native::CrazyCone(n)=>n.update_source(source)?,Native::LavaBall(n)=>n.update_source(source),
            Native::SkyRise(n)=>n.update_source(source),Native::EnergyBall(n)=>n.update_source(source),Native::LegacyTrail(n)=>n.update_source(source),
            Native::Spiral2(n)=>n.update_source(source)?,Native::AParticle(n)=>n.update_source(source)?,Native::Toggle(n)=>n.update_source(source),
            Native::Native301722(n)=>n.update_source(source)?,
            Native::Spiral(n)=>n.update_source(source)?,
            Native::GroundGrid(n)=>n.update_source(source)?,
            Native::ShockWave(n)=>n.update_source(source),
            Native::Stars(n)=>n.update_source(source)?,Native::Suns(n)=>n.update_source(source)?,Native::Electra(n)=>n.update_source(source)?,
            Native::Shield(n)=>n.update_source(source),Native::Sequencer(n)=>n.update_source(source),Native::SkyFlash(n)=>n.update_source(source)?,
            Native::Scatter(n)=>n.update_source(source),Native::Grid(n)=>n.update_source(source)?,Native::Trail(n)=>n.update_source(source)?,_=>{},
        } Ok(())
    }
    pub(super) fn refresh_anchors(&mut self,mut get:impl FnMut(i32)->Option<Mat4>)->Result<()> {
        match &mut self.native {
            Native::Native301722(n)=>{if n.needs_source_root() {if let Some(root)=get(0) {n.set_source_root(root);}}},
            Native::Deformer {effect,..}=>effect.update_anchors(&mut get,self.source),
            Native::Stars(n)=>{n.update_anchors(self.source,self.target)?;for i in 0..n.required_attractors().len() {let id=n.required_attractors()[i];if let Some(m)=get(id) {n.set_attractor(id,m.w_axis.truncate());}}},
            Native::Suns(n)=>{for i in 0..n.required_attractors().len() {let id=n.required_attractors()[i];n.set_attractor(id,get(id));}n.update_hit(self.source.w_axis.truncate(),self.target.w_axis.truncate());},
            Native::Electra(n)=>{for i in 0..n.anchor_ids().len() {let id=n.anchor_ids()[i];n.update_anchor(id,get(id));}},
            _=>{},
        } Ok(())
    }
    pub(super) fn frame(&mut self,dt:f32,renderer:&mut Renderer,mut terrain:Option<&mut dyn FnMut(Vec3)->Option<(Vec3,Vec3)>>)->Result<bool> {
        self.frame_dt=dt;
        // GC10111f91/10110357 resolve the Dynel visual before child work;
        // Trail2's locator check101150b4 also retires an invalid source.
        if self.config.creation==super::Creation::Dynel && !self.source_valid
            && matches!(self.native,Native::Toggle(_)|Native::Scatter(_)|Native::Trail(_)) {
            self.cancel(renderer)?;
            return Ok(false);
        }
        Ok(match &mut self.native {
            Native::Hexagram(n)=>n.frame(dt),
            Native::Drips(n)=>{
                let terrain=terrain.as_mut().ok_or_else(||anyhow::anyhow!("Drips requires actual terrain locator"))?;
                n.advance(dt,self.source_valid,|p|terrain(p).map_or(0.0,|(p,_)|p.y))?
            },
            Native::GroundRing(n)=>n.frame(dt)?,Native::Bubble(n)=>n.frame(dt,&mut renderer.rng)?,Native::Delay(n)=>n.frame(dt,renderer)?,Native::Shadow {effect,..}=>effect.frame(dt)?,
            Native::Vein(n)=>{
                if n.requires_terrain() {let terrain=terrain.as_mut().ok_or_else(||anyhow::anyhow!("Vein requires actual terrain locator"))?;if let Some((point,_))=terrain(self.source.w_axis.truncate()) {n.set_ground_height(point.y);}}
                n.frame(dt,renderer)?
            },
            Native::Splash(n)=>n.frame(dt)?,Native::WaterRipples(n)=>n.frame(dt)?,Native::CrazyCone(n)=>n.frame(dt)?,
            Native::SkyRise(n)=>n.frame(dt)?,Native::EnergyBall(n)=>n.frame(dt)?,Native::LegacyTrail(n)=>n.frame(dt)?,
            Native::Toggle(n)=>n.frame(dt,renderer)?,
            Native::Native301722(n)=>n.frame(dt)?,
            Native::ShockWave(n)=>n.frame(dt)?,
            Native::Highlight(n)=>n.advance(dt),Native::Shield(n)=>n.frame(dt)?,Native::Sequencer(n)=>n.frame(dt,renderer)?,
            Native::SkyFlash(n)=>n.frame(dt)?,Native::Scatter(n)=>n.frame(dt,renderer,terrain)?,
            Native::Shield2 {effect,..}=>{
                let child=effect.take_stop_child();
                if child!=0 {renderer.spawn_configured(super::Binding {effect:child,group:0,attractor:0,note:0,color:0},self.source,self.target.w_axis.truncate(),EffectConfig {creation:super::Creation::Dynel,duration:None,..self.config})?;}
                effect.frame(dt)
            },
            Native::Deformer {effect,child,..}=>{
                let alive=effect.frame(dt)?;
                if effect.morph_child_should_stop() {if let Some(child)=child.take() {renderer.terminate_gracefully(child);}}
                alive
            },
            _=>true,
        })
    }
    pub(super) fn graceful(&mut self,renderer:&mut Renderer)->Result<()> {
        match &mut self.native {
            Native::Hexagram(n)=>n.terminate_gracefully(),Native::Drips(n)=>n.terminate_gracefully(),
            Native::Font(n)=>n.terminate_gracefully(),Native::Fence(n)=>n.terminate_gracefully(),Native::Notum(n)=>n.terminate_gracefully(),
            Native::Vein(n)=>n.graceful(),
            Native::GroundRing(n)=>n.terminate_gracefully(),Native::Bubble(n)=>n.terminate_gracefully(),Native::Delay(n)=>n.terminate_gracefully(renderer),Native::Shadow {effect,..}=>effect.terminate_gracefully(),
            Native::Splash(n)=>n.terminate_gracefully(),Native::WaterRipples(n)=>n.terminate_gracefully(),Native::CrazyCone(n)=>n.terminate_gracefully(),Native::LavaBall(n)=>n.terminate_gracefully(),
            Native::SkyRise(_)|Native::EnergyBall(_)=>{},Native::LegacyTrail(n)=>n.terminate_gracefully(),
            Native::Spiral2(n)=>n.graceful(),Native::AParticle(n)=>n.graceful(),Native::Toggle(n)=>n.graceful(renderer)?,
            Native::Native301722(n)=>n.graceful(),
            Native::GroundGrid(n)=>n.graceful(),
            Native::Spiral(n)=>n.graceful(),Native::Plasma(n)=>n.graceful(),
            Native::ShockWave(n)=>n.terminate_gracefully(),
            Native::Deformer {effect,..}=>effect.terminate_gracefully(),
            Native::Crystal(_)=>unreachable!("class3031 termination deletes centrally"),
            Native::Stars(n)=>n.terminate_gracefully(),Native::Suns(n)=>n.terminate_gracefully(),Native::Electra(n)=>n.terminate_gracefully(),
            Native::Highlight(n)=>n.terminate_gracefully(),Native::Shield(n)=>n.terminate_gracefully(),Native::Sequencer(n)=>n.terminate_gracefully(renderer),
            Native::SkyFlash(n)=>n.terminate_gracefully(),Native::Scatter(n)=>n.graceful(renderer)?,Native::Shield2 {effect,..}=>effect.graceful(),
            Native::Grid(_)=>{},Native::Trail(n)=>n.graceful(),
        } Ok(())
    }
    pub(super) fn cancel(&mut self,renderer:&mut Renderer)->Result<()> {match &mut self.native {Native::Sequencer(n)=>n.cancel(renderer),Native::Scatter(n)=>n.cancel(renderer)?,Native::Toggle(n)=>n.cancel(renderer),Native::Delay(n)=>n.cancel(renderer),Native::Vein(n)=>n.cancel(renderer),Native::Deformer {child,..}=>{if let Some(child)=child.take() {renderer.delete(child);}},_=>{}}Ok(())}
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {
        match &self.native {
            Native::Hexagram(n)=>n.models(),Native::Drips(n)=>vec![(Some(n.material()),(0..8).flat_map(|i|[i*4,i*4+2,i*4+3,i*4,i*4+3,i*4+1]).collect(),32)],
            Native::Font(n)=>n.models().into_iter().filter(|(_,_,count)|*count!=0).collect(),Native::Fence(n)=>n.models(),Native::Notum(n)=>n.models(),
            Native::Vein(n)=>n.models(),
            Native::GroundRing(n)=>n.models(),Native::Bubble(n)=>n.models(),Native::Shadow {indices,count,..}=>if *count==0 {Vec::new()}else{vec![(None,indices.clone(),*count)]},
            Native::Splash(n)=>n.models(),Native::WaterRipples(n)=>n.models(),Native::CrazyCone(n)=>n.models(),Native::LavaBall(n)=>n.models(),
            Native::SkyRise(n)=>n.models(),Native::EnergyBall(n)=>n.models(),Native::LegacyTrail(n)=>n.models(),
            Native::Spiral2(n)=>n.models(),Native::AParticle(n)=>n.models(),
            Native::Native301722(n)=>n.models(),
            Native::GroundGrid(n)=>n.models(),Native::Spiral(n)=>n.models(),Native::Plasma(n)=>n.models(),Native::ShockWave(n)=>n.models(),
            Native::Stars(n)=>n.models(),Native::Suns(n)=>n.models(),Native::Electra(n)=>n.models(),Native::Shield(n)=>n.models(),
            Native::Crystal(n)=>n.models(),
            Native::SkyFlash(n)=>n.models(),Native::Grid(n)=>n.models(),Native::Trail(n)=>n.models(),
            Native::Shield2 {effect,vertices,indices,..}=>{let n=vertices.len();let out=(0..effect.layer_count()).flat_map(|layer|indices.iter().map(move |&i|i+(layer*n) as u32)).collect();vec![(Some(effect.material()),out,n*effect.layer_count())]},
            _=>Vec::new(),
        }
    }
    pub(super) fn blends(&self)->Vec<Blend> {match &self.native {
        Native::Hexagram(n)=>n.blends(),Native::Drips(n)=>vec![n.blend()],
        Native::Font(n)=>n.blends(),Native::Fence(n)=>n.blends(),Native::Notum(n)=>n.blends(),
        Native::Vein(n)=>n.blends(),
        Native::GroundRing(n)=>n.blends(),Native::Bubble(n)=>n.blends(),Native::Shadow {..}=>vec![Blend::AlphaBlend],
        Native::Splash(n)=>n.blends(),Native::WaterRipples(n)=>n.blends(),Native::CrazyCone(n)=>n.blends(),Native::LavaBall(n)=>n.blends(),
        Native::SkyRise(n)=>n.blends(),Native::EnergyBall(n)=>n.blends(),Native::LegacyTrail(n)=>n.blends(),
        Native::Spiral2(n)=>n.blends(),Native::AParticle(n)=>n.blends(),
        Native::Native301722(n)=>n.blends(),
        Native::GroundGrid(n)=>n.blends(),Native::Spiral(n)=>n.blends(),Native::Plasma(n)=>n.blends(),Native::ShockWave(n)=>n.blends(),
        Native::Crystal(n)=>n.blends(),
        Native::Stars(n)=>n.blends(),Native::Suns(n)=>n.blends(),Native::Electra(n)=>n.blends(),Native::Shield(n)=>n.blends(),Native::SkyFlash(n)=>n.blends(),Native::Grid(n)=>n.blends(),Native::Trail(n)=>n.blends(),Native::Shield2 {effect,..}=>vec![effect.blend()],_=>Vec::new(),
    }}
    pub(super) fn apply_material(&self,actors:&mut[ActorFrame]) {match &self.native {Native::Highlight(n)=>n.apply(actors),Native::Deformer {effect,original,..}=>effect.apply_actor(actors,original),_=>{}}}
    #[allow(clippy::too_many_arguments)]
    pub(super) fn vertices(&mut self,time:f32,camera:Vec3,right:Vec3,up:Vec3,gc:&mut R250,ds:&mut R250,crt:&mut CrtRand,terrain:Option<&mut dyn FnMut(Vec3)->Option<(Vec3,Vec3)>>,liquid:Option<&mut dyn FnMut(Vec3)->Option<(f32,i32)>>,collision:Option<&mut dyn FnMut(Vec3,Vec3)->Option<Vec3>>)->Result<Option<Vec<Vec<Vertex>>>> {
        if self.needs_source_mesh() && !self.source_ready {return Ok(Some(Vec::new()));}
        match &mut self.native {
            Native::Hexagram(n)=>{
                let terrain=terrain.ok_or_else(||anyhow::anyhow!("Hexagram requires actual terrain locator"))?;
                n.vertices(self.source_valid,|p|terrain(p).map_or(0.0,|(p,_)|p.y))
            },
            Native::Drips(n)=>Ok(Some(vec![n.vertices(right,up)?])),
            Native::Font(n)=>n.vertices(time,right,up),Native::Fence(n)=>n.vertices(time,crt),
            Native::Notum(n)=>n.vertices(self.frame_dt,self.clock.ok_or_else(||anyhow::anyhow!("Notum requires actual native clock"))?,camera),
            Native::Vein(n)=>n.vertices(time,camera,right,up),
            Native::GroundRing(n)=>{let terrain=terrain.ok_or_else(||anyhow::anyhow!("GroundRing requires actual terrain locator"))?;n.vertices(terrain)},
            Native::Bubble(n)=>n.vertices(right,up),
            Native::Shadow {effect,indices,count}=>{
                let terrain=terrain.ok_or_else(||anyhow::anyhow!("Shadow requires actual terrain locator"))?;
                if let Some((vertices,new_indices))=effect.geometry(terrain)? {*indices=new_indices;*count=vertices.len();Ok(Some(vec![vertices]))}
                else {indices.clear();*count=0;Ok(Some(Vec::new()))}
            },
            Native::Splash(n)=>{let liquid=liquid.ok_or_else(||anyhow::anyhow!("Splash requires actual liquid locator"))?;n.vertices(liquid)},
            Native::WaterRipples(n)=>Ok(Some(n.vertices()?)),
            Native::CrazyCone(n)=>{let terrain=terrain.ok_or_else(||anyhow::anyhow!("CrazyCone requires actual terrain locator"))?;n.vertices(terrain)},
            Native::LavaBall(n)=>n.vertices(time,camera,right,up,crt,terrain,collision),
            Native::SkyRise(n)=>Ok(Some(n.vertices()?)),Native::EnergyBall(n)=>Ok(Some(n.vertices()?)),Native::LegacyTrail(n)=>Ok(Some(n.vertices(camera)?)),
            Native::Spiral2(n)=>n.vertices(time,camera,right,up,gc),Native::AParticle(n)=>n.vertices(time,camera,right,up,gc),
            Native::Native301722(n)=>n.vertices(camera,right,up,gc,crt,terrain),
            Native::Crystal(n)=>n.vertices(time,camera,gc,terrain),
            Native::Spiral(n)=>n.vertices(time),
            Native::Plasma(n)=>n.vertices(time,-right.cross(up),crt),
            Native::GroundGrid(n)=>{let terrain=terrain.ok_or_else(||anyhow::anyhow!("GroundGrid requires actual terrain locator"))?;n.vertices(time,terrain)},
            Native::ShockWave(n)=>{let terrain=terrain.ok_or_else(||anyhow::anyhow!("ShockWave requires actual terrain locator"))?;n.vertices(terrain)},
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

#[cfg(test)]
mod tests {
    #[test]
    fn retail_nonvisual_child_controls_retire_before_spawning() -> Result<()> {
        let dir=ao_gui::client_dir();
        if !dir.join("rdb.db").exists() {return Ok(());}
        let mut renderer=Renderer::open(&dir)?;
        let (mut gc,mut ds,mut crt)=(R250::new(7),R250::new(8),CrtRand::new(1));
        for kind in [3029,3036,3039] {
            let template=renderer.templates.by_id.values().find(|t|t.kind==kind)
                .ok_or_else(||anyhow::anyhow!("missing installed class{kind}"))?.clone();
            let config=EffectConfig {creation:super::super::Creation::Dynel,source_identity:Some((50000,1)),source_nonvisual:true,..Default::default()};
            let mut buff=Buff::new(&template,Mat4::IDENTITY,Vec3::ZERO,0,config,&mut gc,&mut ds,&mut crt,None,None)?;
            assert!(!buff.frame(0.0,&mut renderer,None)?,"class{kind} must reject its unavailable visual before environment/child work");
            assert!(renderer.active.is_empty(),"class{kind} spawned a child from an invalid source");
            let mut buff=Buff::new(&template,Mat4::IDENTITY,Vec3::ZERO,0,EffectConfig {source_nonvisual:false,..config},&mut gc,&mut ds,&mut crt,None,None)?;
            buff.source_removed();
            assert!(!buff.frame(0.0,&mut renderer,None)?,"class{kind} retained a deleted source");
        }
        Ok(())
    }
    use super::*;
    #[test]
    fn authored_groundgrid_wrapper_terrain_and_stop() {
        let t=Template {kind:3030,words:vec![57859,0,0,0,0,0,0,0,1092616192,58,20,2,1077936128,1077936128,1073741824,1073741824,4286628095,4286628095,1073741824,1048576000,1084227584,1036831949,1036831949,1056964608,1056964608,0,0]};
        let (mut gc,mut ds,mut crt)=(R250::new(7),R250::new(8),CrtRand::new(1));
        let mut buff=Buff::new(&t,Mat4::IDENTITY,Vec3::ZERO,0,EffectConfig::default(),&mut gc,&mut ds,&mut crt,None,None).unwrap();
        assert!(Buff::supports(3030));
        assert!(!buff.needs_source_mesh());
        assert_eq!(buff.models()[0].2,760);
        assert!(buff.vertices(1.0,Vec3::ZERO,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt,None,None,None).is_err());
        let mut terrain=|p:Vec3|Some((Vec3::new(p.x,2.0,p.z),Vec3::Y));
        let groups=buff.vertices(1.0,Vec3::ZERO,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt,Some(&mut terrain),None,None).unwrap().unwrap();
        assert_eq!(groups[0].len(),760);
        if let Native::GroundGrid(n)=&mut buff.native {n.graceful();}
        assert!(buff.vertices(1.0,Vec3::ZERO,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt,Some(&mut terrain),None,None).unwrap().is_none());
    }
    #[test]
    fn authored_deformer_wrapper_has_no_dummy_geometry() {
        let t=Template {kind:3001,words:vec![0,0,0,0,0,0,0,0,3212836864,0,0,1065353216,1073741824,1073741824,3,1017,2004,1077936128,0,1039516303,1031127695,1017,2004,1078774989,1051260355,1039516303,1031127695,1017,2004,1079613850,1059816735,1039516303,1031127695]};
        let (mut gc,mut ds,mut crt)=(R250::new(7),R250::new(8),CrtRand::new(1));
        let mut buff=Buff::new(&t,Mat4::IDENTITY,Vec3::ZERO,0,EffectConfig::default(),&mut gc,&mut ds,&mut crt,None,None).unwrap();
        assert!(buff.models().is_empty());
        assert!(buff.blends().is_empty());
        assert!(buff.morph_scene().is_none());
        assert!(buff.morph_child().is_none());
        assert!(buff.needs_source_mesh());
        buff.prepare_source(&Scene::default(),&ActorFrame::default(),&mut crt).unwrap();
        assert!(buff.source_ready);
        assert!(buff.needs_source_pose(),"host deformation consumes every live pose, not only its first pose");
        assert!(buff.vertices(0.0,Vec3::ZERO,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt,None,None,None).unwrap().unwrap().is_empty());
    }
    #[test]
    #[ignore = "requires installed retail effects and resources"]
    fn retail_buff_dispatch_groundgrid_lifecycle() {
        let mut renderer=Renderer::open(&ao_gui::client_dir()).unwrap();
        let binding=super::super::Binding {group:0,attractor:0,effect:71230,note:0,color:0};
        let handle=renderer.spawn_configured(binding,Mat4::IDENTITY,Vec3::ZERO,EffectConfig {creation:super::super::Creation::Matrix,..Default::default()}).unwrap();
        assert!(renderer.is_active(handle));
        assert!(!renderer.needs_source_mesh((50000,1)));
        renderer.terminate_gracefully(handle);
        assert!(!renderer.is_active(handle),"GroundGrid uses immediate native termination");
    }
}
