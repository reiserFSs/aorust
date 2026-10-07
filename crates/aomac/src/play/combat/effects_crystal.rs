//! GfxControlTParticle2: GC101143bb/101131ec/10113dbf/10113604;
//! GfxVisualTParticle2: DS1002a9e7/1002ac11. Two crossed world-space strips.
use super::{materials, quad, sprites, EffectConfig, Template};
use anyhow::{ensure, Context, Result};
use ao_formats::weather::R250;
use ao_scene::{Blend, Vertex};
use glam::{Mat4, Vec3};
fn f(t:&Template,i:usize)->f32 {f32::from_bits(t.words.get(i).copied().unwrap_or(0))}
fn sample(t:&Template,a:usize,b:usize,r:&mut R250)->f32 {f(t,a)+(f(t,b)-f(t,a))*super::random_fraction(r)}
fn color(t:&Template,start:usize,phase:f32)->[f32;4] {
    let n=t.words.get(start).copied().unwrap_or(0) as usize;
    let rgba=|c:u32| {let [a,r,g,b]=c.to_be_bytes();[r as f32/255.0,g as f32/255.0,b as f32/255.0,a as f32/255.0]};
    if n==0 {return [1.0;4];}
    let mut out=rgba(t.words[start+n*2]);
    for i in 0..n.saturating_sub(1) {let a=f(t,start+1+i*2);let b=f(t,start+3+i*2);if a<phase && phase<b {let x=rgba(t.words[start+2+i*2]);let y=rgba(t.words[start+4+i*2]);let u=(phase-a)/(b-a);out=std::array::from_fn(|j|x[j]+(y[j]-x[j])*u);break;}}
    for v in &mut out[..3] {*v=v.powf(2.2);}out
}
struct Particle {position:Vec3,velocity:Vec3,acceleration:Vec3,life:f32,remaining:f32,size:f32,frame:f32}
pub(super) struct Crystal {template:Template,source:Mat4,particles:Vec<Particle>,previous:f32,emission:f32,duration:f32,tail_curve:usize,frame_rate:f32}
impl Crystal {
    pub(super) fn new(t:&Template,source:Mat4,r:&mut R250)->Result<Self> {
        ensure!(t.kind==3031,"not TParticle2");
        for i in 1..34 {if ![7,9,10,11,15,32].contains(&i) {ensure!(f(t,i).is_finite(),"nonfinite TParticle2 parameter {i}");}}
        ensure!(f(t,25)>0.0 && f(t,26)>0.0 && f(t,33)>0.0,"invalid TParticle2 lifetime/aspect");
        let mut at=34;
        for _ in 0..2 {let n=t.word(at)? as usize;ensure!(n<=t.words.len().saturating_sub(at+1)/2,"truncated TParticle2 colour curve");for j in 0..n {ensure!(f(t,at+1+j*2).is_finite(),"nonfinite TParticle2 colour knot");}at+=1+n*2;}
        materials::MATERIALS.get(t.words.get(10).copied().unwrap_or(0) as usize).context("unknown TParticle2 material")?;
        let n=t.words.get(11).copied().unwrap_or(0) as usize;ensure!(n<=u16::MAX as usize/8,"TParticle2 index overflow");
        let mut out=Self {template:t.clone(),source:sprites::connector(t,source)?,particles:Vec::with_capacity(n),previous:0.0,emission:0.0,duration:f(t,8),tail_curve:35+t.words.get(34).copied().unwrap_or(0) as usize*2,frame_rate:f(t,31)};
        for _ in 0..n {let mut p=out.emit(r);if t.words.get(9).copied().unwrap_or(0) == 1 {p.remaining=0.0;}out.particles.push(p);}Ok(out)
    }
    fn emit(&self,r:&mut R250)->Particle {
        let t=&self.template;let size=sample(t,27,28,r);let life=sample(t,25,26,r);
        let material=materials::MATERIALS[t.words.get(10).copied().unwrap_or(0) as usize];
        let frame=if t.words.first().copied().unwrap_or(0) & 0x20000!=0 {material.3 as f32+(material.4-material.3) as f32*super::random_fraction(r)}else{0.0};
        let position=self.source.w_axis.truncate()+Vec3::new(super::random_fraction(r)*2.0-1.0,super::random_fraction(r)*2.0-1.0,-(super::random_fraction(r)*2.0-1.0))*f(t,13);
        let velocity=Vec3::new(sample(t,17,18,r),sample(t,19,20,r),-sample(t,21,22,r));
        // Native consumes angle and spin samples even though this visual uses velocity axes.
        if t.words.first().copied().unwrap_or(0) & 0x4000==0 {super::random_fraction(r);}sample(t,23,24,r);
        let rotation=Mat4::from_cols(self.source.x_axis.truncate().normalize_or_zero().extend(0.0),self.source.y_axis.truncate().normalize_or_zero().extend(0.0),self.source.z_axis.truncate().normalize_or_zero().extend(0.0),Vec3::ZERO.extend(1.0));
        Particle {position,velocity:rotation.transform_vector3(velocity),acceleration:rotation.transform_vector3(Vec3::Y*f(t,16)),life,remaining:life,size,frame}
    }
    pub(super) fn configure(&mut self,c:EffectConfig) {if let Some(d)=c.duration {self.duration=d;}}
    pub(super) fn update_source(&mut self,m:Mat4)->Result<()> {self.source=sprites::connector(&self.template,m)?;Ok(())}
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {let n=self.particles.len()*2;vec![(Some(self.template.words.get(10).copied().unwrap_or(0) as usize),(0..n as u32).flat_map(|i|[i*4,i*4+1,i*4+2,i*4+2,i*4+1,i*4+3]).collect(),n*4)]}
    pub(super) fn blends(&self)->Vec<Blend> {vec![if self.template.words.first().copied().unwrap_or(0)&0x200!=0 {Blend::Additive}else{Blend::AlphaBlend}]}
    pub(super) fn vertices(&mut self,time:f32,camera:Vec3,r:&mut R250,mut terrain:Option<&mut dyn FnMut(Vec3)->Option<(Vec3,Vec3)>>)->Result<Option<Vec<Vec<Vertex>>>> {
        let initial=self.previous==0.0;let dt=(time-self.previous).max(0.0);self.previous=time;self.emission+=dt;
        if self.duration>=0.0 && time>self.duration {return Ok(None);}
        if self.template.words.first().copied().unwrap_or(0) & 0x400!=0 {
            let query=terrain.as_mut().context("TParticle2 ground snapping requires terrain")?;
            let old_y=self.source.w_axis.y;
            self.source.w_axis.y=query(self.source.w_axis.truncate()).context("missing TParticle2 source ground")?.0.y;
            if initial {for p in &mut self.particles {p.position.y+=self.source.w_axis.y-old_y;}}
        }
        let t=&self.template;let mut out=Vec::with_capacity(self.particles.len()*8);let mut alive=0;
        for p in &mut self.particles {
            p.remaining-=dt;
            if p.remaining>0.0 {
                alive+=1;
                if t.words.first().copied().unwrap_or(0) & 0x8000!=0 {
                    let query=terrain.as_mut().context("TParticle2 bounce requires terrain")?;
                    if p.position.y<=query(p.position).context("missing TParticle2 particle ground")?.0.y {
                        p.velocity*=f(t,29);p.velocity.y= -p.velocity.y;
                        // GC10113604 resamples angular velocity although DS does not render it.
                        sample(t,23,24,r);
                    }
                }
                p.position+=p.velocity*dt;p.velocity+=p.acceleration*dt;p.size*=f(t,30);
                let material=materials::MATERIALS[t.words.get(10).copied().unwrap_or(0) as usize];
                if t.words.get(32).copied().unwrap_or(0)!=0 && p.frame!=-999.0 {
                    p.frame+=self.frame_rate*dt;
                    if p.frame>material.4 as f32 {match t.words.get(32).copied().unwrap_or(0) { 1=>p.frame=material.3 as f32,2=>p.frame= -999.0,3=>{self.frame_rate= -self.frame_rate;p.frame+=self.frame_rate*dt;},_=>{} }}
                    if p.frame<material.3 as f32 && t.words.get(32).copied().unwrap_or(0) == 3 {self.frame_rate= -self.frame_rate;p.frame+=self.frame_rate*dt;}
                }
            }
            let phase=1.0-p.remaining/p.life;let head=color(t,34,phase);let tail=color(t,self.tail_curve,phase);
            let end=p.position-p.velocity*f(t,14);let direction=p.velocity.normalize_or_zero();let side=direction.cross(Vec3::Y).normalize_or_zero();let side=if side==Vec3::ZERO {Vec3::X}else{side};let other=side.cross(direction).normalize_or_zero();
            for axis in [other,side] {
                let positions=[p.position-axis*p.size,p.position+axis*p.size,end-axis*p.size/f(t,33),end+axis*p.size/f(t,33)];
                let material=materials::MATERIALS[t.words.get(10).copied().unwrap_or(0) as usize];
                let frame=p.frame as i32;let u=frame.rem_euclid(material.1 as i32) as f32/material.1 as f32;let v=(frame/material.1 as i32) as f32/material.2 as f32;
                let du=1.0/material.1 as f32;let dv=1.0/material.2 as f32;
                let uv=if t.words.first().copied().unwrap_or(0) & 0x100!=0 {[[u,v],[u,v+dv],[u+du,v],[u+du,v+dv]]}else{[[u,v+dv],[u+du,v+dv],[u,v],[u+du,v]]};
                let uv=if t.words.first().copied().unwrap_or(0) & 0x80000!=0 {[uv[2],uv[3],uv[0],uv[1]]}else{uv};
                quad(&mut out,positions,uv,[0.0;4]);
                let base=out.len()-4;if p.remaining>0.0 {for (j,mut c) in [head,head,tail,tail].into_iter().enumerate() {
                    // DS1002aaa3 attenuates each strip by its facing to the camera.
                    if t.words.first().copied().unwrap_or(0) & 0x40000!=0 {c[3]*=axis.cross(direction).normalize_or_zero().dot((camera-p.position).normalize_or_zero()).abs();}
                    out[base+j].color=c;
                }}
            }
        }
        if t.words.get(9).copied().unwrap_or(0) == 0 && alive==0 {return Ok(None);}
        if t.words.get(9).copied().unwrap_or(0) == 1 && time<self.duration-f(t,26) && f(t,12)>0.0 && self.emission>f(t,12) {
            let count=t.words.get(15).copied().unwrap_or(0) as usize;
            let mut emitted=0;
            for i in 0..self.particles.len() {if self.particles[i].remaining<=0.0 {
                let mut p=self.emit(r);
                if self.template.words.first().copied().unwrap_or(0) & 0x10000!=0 {let query=terrain.as_mut().context("TParticle2 ground emission requires terrain")?;p.position.y=query(p.position).context("missing TParticle2 emission ground")?.0.y;}
                self.particles[i]=p;self.emission=0.0;emitted+=1;if emitted>=count {break;}
            }}
        }
        Ok(Some(vec![out]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authored_crystal_73001_terrain_modes() {
        let templates=super::super::Templates::open(&ao_gui::client_dir()).unwrap();
        let mut t=templates.by_id[&73001].clone();t.words[0]|=0x400|0x8000|0x10000|0x4000|0x20000;t.words[32]=3;
        let mut gc=R250::new(0xe6f1);
        let mut effect=Crystal::new(&t,Mat4::IDENTITY,&mut gc).unwrap();
        let mut ground=|p:Vec3|Some((Vec3::new(p.x,2.0,p.z),Vec3::Y));
        effect.vertices(0.016,Vec3::Z,&mut gc,Some(&mut ground)).unwrap();
        assert!(effect.particles.iter().filter(|p|p.remaining>0.0).all(|p|p.position.y==2.0));
    }
    #[test]
    #[ignore="installed authored assets and offscreen GPU regression"]
    fn retail_crystal_authored_modes_frames()->Result<()> {
        use super::super::{Binding,Renderer,MODEL_BASE};
        let out=std::env::var_os("AOMAC_EFFECT_FRAMES").context("AOMAC_EFFECT_FRAMES required")?;
        let out=std::path::PathBuf::from(out);std::fs::create_dir_all(&out)?;
        let mut renderer=Renderer::open(&ao_gui::client_dir())?;
        let origin=Vec3::new(5000.0,10.0,5000.0);
        let mut host=ao_render::Host::headless();
        let ids:Vec<_>=renderer.templates.by_id.iter().filter_map(|(&id,t)|(t.kind==3031).then_some(id)).collect();
        ensure!(!ids.is_empty(),"no authored TParticle2 records");
        for id in ids {
            renderer.clear();
            let t=&renderer.templates.by_id[&id];
            let life=f(t,25).max(f(t,26));
            let speed=Vec3::new(f(t,17).abs().max(f(t,18).abs()),f(t,19).abs().max(f(t,20).abs()),f(t,21).abs().max(f(t,22).abs())).length();
            let trail=f(t,14).abs();
            let acceleration=f(t,16).abs();
            let width=f(t,27).abs().max(f(t,28).abs())*f(t,30).max(1.0).powf((life*60.0).ceil())/f(t,33).min(1.0);
            let radius=f(t,13).abs()*3.0f32.sqrt()+speed*(life+trail)+acceleration*life*(life*0.5+trail)+width;
            // Same sphere framing as ao_render::default_view (60-degree FOV).
            // The old 5.74-unit eye sat inside authored 16-unit tail widths.
            let eye=origin+Vec3::new(2.0,2.0,5.0).normalize()*radius.max(1.0)*2.4;
            host.camera=ao_render::Camera::look_at(eye,origin);
            renderer.spawn(Binding {group:0,attractor:0,effect:id,note:0,color:0},super::super::Creation::Vector,Mat4::from_translation(origin),origin+Vec3::X)?;
            for frame in 1..=60 {host.actors.clear();let mut ground=|p:Vec3|Some((Vec3::new(p.x,10.0,p.z),Vec3::Y));renderer.frame(1.0/60.0,&mut host,Some(&mut ground));
                if [1,6,15,30,45,60].contains(&frame) {let models:Vec<_>=renderer.models.iter().map(|(&id,m)|(MODEL_BASE|id as u32 as u64,m.scene.clone())).collect();
                    ao_render::render_to_png_actors(&ao_scene::Scene::default(),&models,host.actors.clone(),eye.to_array(),origin.to_array(),640,480,&out.join(format!("crystal3031_{id}_{frame}.png")),frame as f32/60.0)?;
                }
            }
        }Ok(())
    }
    #[test]
    fn crystal_is_native_crossed_emitter_not_particle_or_grid_alias() {
        let mut words=vec![0;36];words[0]=515;words[9]=1;words[10]=74;words[11]=80;words[15]=3;
        for (i,v) in [(8,5.0f32),(12,0.001),(14,1.0),(17,-5.0),(18,5.0),(19,-5.0),(20,5.0),(21,-5.0),(22,5.0),(25,0.15),(26,0.4),(27,4.0),(28,8.0),(30,1.01),(33,0.5)] {words[i]=v.to_bits();}
        let t=Template {kind:3031,words};let mut rng=R250::new(0xe6f1);
        let mut effect=Crystal::new(&t,Mat4::IDENTITY,&mut rng).unwrap();
        assert!(super::super::buffs::Buff::supports(3031));
        assert_eq!(effect.models()[0].2,640);assert_eq!(effect.models()[0].1.len(),960);
        effect.vertices(0.016,Vec3::Z,&mut rng,None).unwrap();
        assert_eq!(effect.particles.iter().filter(|p|p.remaining>0.0).count(),3);
        let vertices=effect.vertices(0.032,Vec3::Z,&mut rng,None).unwrap().unwrap();
        assert!(vertices[0].iter().any(|v|v.color[3]>0.0));
        let mut truncated=t;truncated.words[34]=100;assert!(Crystal::new(&truncated,Mat4::IDENTITY,&mut rng).is_err());
    }
    #[test]
    fn crystal_graceful_deletes_without_advancing_frame_time() {
        use super::super::{Binding,Renderer};
        let dir=ao_gui::client_dir();
        if !dir.join("Setupf/gfxtweak.bin").exists() || !dir.join("cd_image/rdb.db").exists() {return;}
        let mut renderer=Renderer::open(&dir).unwrap();
        let handle=renderer.spawn_configured(Binding {group:0,attractor:0,effect:73001,note:0,color:0},Mat4::IDENTITY,Vec3::ZERO,EffectConfig {creation:super::super::Creation::Vector,..Default::default()}).unwrap();
        assert!(renderer.active.iter().any(|a|a.actor==handle));
        renderer.terminate_gracefully(handle);
        assert!(!renderer.active.iter().any(|a|a.actor==handle),"termination must not wait for time>duration");
    }
}
