//! Native vector-created controls: GC SpinningShot1011 (100f41c5/100f45e2,
//! 100f4053/100f3d16/100f3cab) and FallSteam3007 (100da927/100da6da/100da691).
use super::{Binding, EffectConfig, Renderer, Template, sprites};
use anyhow::{ensure, Result};
use ao_formats::weather::R250;
use ao_scene::{Blend, Vertex};
use glam::{Mat4, Vec3};
#[path = "effects_fall_steam.rs"]
mod fall_steam;

pub(super) struct ReplicatedEffect {
    template: Template,
    config: EffectConfig,
    target: Mat4,
    center: Vec3,
    point: Vec3,
    children: [u32;2],
    elapsed: f32,
    ticks: u32,
    target_valid: bool,
    steam: Option<fall_steam::FallSteam>,
}
impl ReplicatedEffect {
    pub(super) fn supports(kind:i32)->bool {matches!(kind,1011|3007)}
    pub(super) fn new(t:&Template,source:Mat4,target:Vec3,config:EffectConfig)->Result<Self> {
        ensure!(Self::supports(t.kind),"unsupported replicated class {}",t.kind);
        for i in 1..=6 {t.float(i)?;}
        t.float(8)?;
        if t.kind==1011 {for i in 12..=25 {t.float(i)?;}}
        let center=source.w_axis.truncate();
        Ok(Self {template:t.clone(),config,target:Mat4::from_translation(target),center,point:center,children:[0;2],elapsed:0.0,ticks:0,target_valid:true,steam:if t.kind==3007 {Some(fall_steam::FallSteam::new(t,center,target)?)}else{None}})
    }
    pub(super) fn target_attractor(&self)->i32 {self.template.words.get(7).copied().unwrap_or(0) as i32}
    // GC100f3aee/100f3afa forward position/matrix setters to the target connector.
    // FallSteam's corresponding native slots are no-ops.
    pub(super) fn update_source(&mut self,source:Mat4) {if self.template.kind==1011 {self.target=source;}}
    pub(super) fn update_position(&mut self,point:Vec3) {if self.template.kind==1011 {self.target=Mat4::from_translation(point);}}
    pub(super) fn update_target(&mut self,target:Mat4) {self.target=target;}
    pub(super) fn invalidate_target(&mut self) {self.target_valid=false;}
    fn destination(&self)->Result<Vec3> {
        // Connector bit2 returns zero position (GC10105eb4), not its authored target.
        if self.template.word(0)?&2!=0 {return Ok(Vec3::ZERO);}
        Ok(sprites::connector(&self.template,self.target)?.w_axis.truncate())
    }
    fn initial_point(&self)->Result<Vec3> {
        let d=self.destination()?-self.center;
        let (radial,side)=if d.x==0.0 && d.z==0.0 {(Vec3::Z,Vec3::X)}else {
            (Vec3::new(d.x,0.0,d.z).normalize(),Vec3::new(-d.z,0.0,d.x).normalize())
        };
        let angle=self.template.float(22)?;
        Ok(self.center+self.template.float(21)?*(radial*angle.cos()+side*angle.sin()))
    }
    pub(super) fn initialize(&mut self,renderer:&mut Renderer)->Result<()> {
        if self.template.kind!=1011 {return Ok(());}
        self.point=self.initial_point()?;
        let color=|i|->Result<[f32;4]> {Ok([self.template.float(i+1)?,self.template.float(i+2)?,self.template.float(i+3)?,self.template.float(i)?])};
        // GC100f4053 creates both children with the Vector overload.
        let config=EffectConfig {creation:super::Creation::Vector,start_color:Some(self.config.start_color.unwrap_or(color(12)?)),stop_color:Some(self.config.stop_color.unwrap_or(color(16)?)),..Default::default()};
        for i in 0..2 {
            let id=self.template.word(10+i)? as i32;
            if id==0 {continue;}
            match renderer.spawn_configured(Binding {group:0,attractor:0,effect:id,note:0,color:0},Mat4::from_translation(self.point),self.point,config) {
                Ok(handle)=>self.children[i]=handle,
                Err(error)=>{self.cancel(renderer);return Err(error);}
            }
        }
        Ok(())
    }
    pub(super) fn cancel(&mut self,renderer:&mut Renderer) {
        // Native destructor deletes both owned children; it does not fade them.
        for handle in &mut self.children {if *handle!=0 {renderer.delete(*handle);*handle=0;}}
    }
    fn advance_shot(&mut self,dt:f32)->Result<()> {
        let direction=(self.destination()?-self.center).normalize_or_zero();
        let radial=self.point-self.center;
        let cross=-direction.cross(radial);
        self.center+=direction*self.template.float(20)?*dt;
        let radial=if radial==Vec3::ZERO {Vec3::X}else{radial.normalize()};
        let cross=if cross==Vec3::ZERO {Vec3::Y}else{cross.normalize()};
        // GC100f3e6c multiplies angular speed by this frame's dt, not total age.
        let angle=self.template.float(23)?*dt;
        self.point=self.center+self.template.float(21)?*(radial*angle.cos()+cross*angle.sin());
        Ok(())
    }
    pub(super) fn frame(&mut self,dt:f32,renderer:&mut Renderer)->Result<bool> {
        ensure!(dt.is_finite() && dt>=0.0,"invalid replicated frame time");
        let step=match self.ticks {0=>0.0,1=>dt.min(0.033),_=>dt};
        self.ticks=self.ticks.saturating_add(1);self.elapsed+=step;
        if !self.target_valid {self.cancel(renderer);return Ok(false);}
        let duration=self.config.duration.unwrap_or(self.template.float(8)?);
        if duration>0.0 && self.elapsed>duration {self.cancel(renderer);return Ok(false);}
        if self.template.kind==1011 {
            self.advance_shot(step)?;
            for handle in self.children {if handle!=0 {renderer.update_position(handle,self.point);}}
        }
        Ok(true)
    }
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {self.steam.as_ref().map_or_else(Vec::new,|s|s.models())}
    pub(super) fn blends(&self)->Vec<Blend> {self.steam.as_ref().map_or_else(Vec::new,|s|s.blends())}
    pub(super) fn write_vertices(&mut self,time:f32,right:Vec3,up:Vec3,ds:&mut R250,out:&mut Vec<Vertex>)->Result<()> {
        if let Some(s)=&mut self.steam {s.write_vertices(time,right,up,ds,out)?;}
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn short_spinning_shot_defaults_to_zero() -> Result<()> {
        let mut e=ReplicatedEffect::new(&Template {kind:1011,words:vec![]},Mat4::IDENTITY,Vec3::Z,EffectConfig::default())?;
        assert_eq!(e.target_attractor(),0);assert_eq!(e.initial_point()?,Vec3::ZERO);
        e.advance_shot(0.1)?;assert_eq!(e.center,Vec3::ZERO);assert_eq!(e.point,Vec3::ZERO);Ok(())
    }
    #[test]
    fn authored_9010_spinning_shot_uses_frame_angle_and_live_vector_target()->Result<()> {
        let t=Template {kind:1011,words:vec![5,0,0,0,0,0,0,0,1090519040,u32::MAX,6203,8003,1065353216,1048576000,1056964608,1065185444,0,1048576000,1056964608,1065185444,1092616192,1065353216,1070141403,1086918619,1065353216,1065353216]};
        let mut e=ReplicatedEffect::new(&t,Mat4::IDENTITY,Vec3::Z*20.0,EffectConfig::default())?;
        e.point=e.initial_point()?;let initial=e.point;
        e.advance_shot(0.1)?;
        assert!((e.center.z-1.0).abs()<1e-6);
        assert!(((e.point-e.center).length()-1.0).abs()<1e-6);
        e.update_target(Mat4::from_translation(Vec3::X*20.0));
        e.advance_shot(0.1)?;assert!(e.center.x>0.0);assert_ne!(e.point,initial);
        assert_eq!(e.target_attractor(),0);assert!(e.models().is_empty());
        Ok(())
    }
    #[test]
    fn authored_9010_parent_delete_and_target_loss_delete_actual_children()->Result<()> {
        let dir=ao_gui::client_dir();
        if !dir.join("Setupf/gfxtweak.bin").exists() || !dir.join("cd_image/rdb.db").exists() {return Ok(());}
        let mut renderer=Renderer::open(&dir)?;
        let binding=Binding {group:0,attractor:0,effect:9010,note:0,color:0};
        let identity=(50000,77);
        let config=EffectConfig {creation:super::super::Creation::VectorDynel,target_identity:Some(identity),..Default::default()};
        assert!(renderer.spawn_configured(binding,Mat4::IDENTITY,Vec3::Z*20.0,config).is_err());
        assert!(renderer.active.is_empty());
        renderer.prepare_anchor(identity,0,Some(Mat4::from_translation(Vec3::Z*20.0)));
        let parent=renderer.spawn_configured(binding,Mat4::IDENTITY,Vec3::Z*20.0,config)?;
        let children=renderer.active.iter().find(|a|a.actor==parent).unwrap().native_replicated.as_ref().unwrap().children;
        assert!(children.iter().all(|h|*h!=0 && renderer.is_active(*h)));
        renderer.delete(parent);
        assert!(!renderer.is_active(parent));
        assert!(children.iter().all(|h|!renderer.is_active(*h)));
        renderer.prepare_anchor(identity,0,Some(Mat4::from_translation(Vec3::Z*20.0)));
        let parent=renderer.spawn_configured(binding,Mat4::IDENTITY,Vec3::Z*20.0,config)?;
        let children=renderer.active.iter().find(|a|a.actor==parent).unwrap().native_replicated.as_ref().unwrap().children;
        renderer.refresh_anchors(|_,_|None);
        let mut host=ao_render::Host::headless();
        renderer.frame(0.016,&mut host,None);
        assert!(!renderer.is_active(parent));
        assert!(children.iter().all(|h|!renderer.is_active(*h)));
        Ok(())
    }
}
