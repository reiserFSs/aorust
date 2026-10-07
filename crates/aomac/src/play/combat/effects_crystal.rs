//! GfxControlTParticle2: GC101143bb/101131ec/10113dbf/10113604;
//! GfxVisualTParticle2: DS1002a9e7/1002ac11. Two crossed world-space strips.
use super::{materials, quad, sprites, EffectConfig, Template};
use anyhow::{ensure, Context, Result};
use ao_formats::weather::R250;
use ao_scene::{Blend, Vertex};
use glam::{Mat4, Vec3};
fn f(t:&Template,i:usize)->f32 {f32::from_bits(t.words[i])}
fn sample(t:&Template,a:usize,b:usize,r:&mut R250)->f32 {f(t,a)+(f(t,b)-f(t,a))*super::random_fraction(r)}
fn color(t:&Template,start:usize,phase:f32)->[f32;4] {
    let n=t.words[start] as usize;
    let rgba=|c:u32| {let [a,r,g,b]=c.to_be_bytes();[r as f32/255.0,g as f32/255.0,b as f32/255.0,a as f32/255.0]};
    if n==0 {return [1.0;4];}
    let mut out=rgba(t.words[start+n*2]);
    for i in 0..n.saturating_sub(1) {let a=f(t,start+1+i*2);let b=f(t,start+3+i*2);if a<phase && phase<b {let x=rgba(t.words[start+2+i*2]);let y=rgba(t.words[start+4+i*2]);let u=(phase-a)/(b-a);out=std::array::from_fn(|j|x[j]+(y[j]-x[j])*u);break;}}
    for v in &mut out[..3] {*v=v.powf(2.2);}out
}
struct Particle {position:Vec3,velocity:Vec3,life:f32,remaining:f32,size:f32}
pub(super) struct Crystal {template:Template,source:Mat4,particles:Vec<Particle>,previous:f32,emission:f32,duration:f32,tail_curve:usize}
impl Crystal {
    pub(super) fn new(t:&Template,source:Mat4,r:&mut R250)->Result<Self> {
        ensure!(t.kind==3031,"not TParticle2");t.word(34)?;
        ensure!(t.words[0]&!(0x203)==0,"unsupported TParticle2 environment/visual flags");
        ensure!(t.words[9]<=1 && t.words[32]==0,"unsupported TParticle2 emission/atlas mode");
        ensure!(f(t,23)==0.0 && f(t,24)==0.0,"unsupported TParticle2 angular motion");
        for i in 1..34 {if ![7,9,10,11,15,32].contains(&i) {ensure!(f(t,i).is_finite(),"nonfinite TParticle2 parameter {i}");}}
        ensure!(f(t,25)>0.0 && f(t,26)>0.0 && f(t,33)>0.0,"invalid TParticle2 lifetime/aspect");
        let mut at=34;
        for _ in 0..2 {let n=t.word(at)? as usize;ensure!(n<=t.words.len().saturating_sub(at+1)/2,"truncated TParticle2 colour curve");for j in 0..n {ensure!(f(t,at+1+j*2).is_finite(),"nonfinite TParticle2 colour knot");}at+=1+n*2;}
        materials::MATERIALS.get(t.words[10] as usize).context("unknown TParticle2 material")?;
        let n=t.words[11] as usize;ensure!(n<=u16::MAX as usize/8,"TParticle2 index overflow");
        let mut out=Self {template:t.clone(),source:sprites::connector(t,source)?,particles:Vec::with_capacity(n),previous:0.0,emission:0.0,duration:f(t,8),tail_curve:35+t.words[34] as usize*2};
        for _ in 0..n {let mut p=out.emit(r);if t.words[9]==1 {p.remaining=0.0;}out.particles.push(p);}Ok(out)
    }
    fn emit(&self,r:&mut R250)->Particle {
        let t=&self.template;let size=sample(t,27,28,r);let life=sample(t,25,26,r);
        let position=self.source.w_axis.truncate()+Vec3::new(super::random_fraction(r)*2.0-1.0,super::random_fraction(r)*2.0-1.0,-(super::random_fraction(r)*2.0-1.0))*f(t,13);
        let velocity=Vec3::new(sample(t,17,18,r),sample(t,19,20,r),-sample(t,21,22,r));
        // Native consumes angle and spin samples even though this visual uses velocity axes.
        super::random_fraction(r);sample(t,23,24,r);
        let rotation=Mat4::from_cols(self.source.x_axis.truncate().normalize_or_zero().extend(0.0),self.source.y_axis.truncate().normalize_or_zero().extend(0.0),self.source.z_axis.truncate().normalize_or_zero().extend(0.0),Vec3::ZERO.extend(1.0));
        Particle {position,velocity:rotation.transform_vector3(velocity),life,remaining:life,size}
    }
    pub(super) fn configure(&mut self,c:EffectConfig) {if let Some(d)=c.duration {self.duration=d;}}
    pub(super) fn update_source(&mut self,m:Mat4)->Result<()> {self.source=sprites::connector(&self.template,m)?;Ok(())}
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {let n=self.particles.len()*2;vec![(Some(self.template.words[10] as usize),(0..n as u32).flat_map(|i|[i*4,i*4+1,i*4+2,i*4+2,i*4+1,i*4+3]).collect(),n*4)]}
    pub(super) fn blends(&self)->Vec<Blend> {vec![if self.template.words[0]&0x200!=0 {Blend::Additive}else{Blend::AlphaBlend}]}
    pub(super) fn vertices(&mut self,time:f32,r:&mut R250)->Result<Option<Vec<Vec<Vertex>>>> {
        let dt=(time-self.previous).max(0.0);self.previous=time;self.emission+=dt;
        if self.duration>=0.0 && time>self.duration {return Ok(None);}
        let t=&self.template;let mut out=Vec::with_capacity(self.particles.len()*8);let mut alive=0;
        for p in &mut self.particles {
            p.remaining-=dt;
            if p.remaining>0.0 {alive+=1;p.position+=p.velocity*dt;p.velocity.y+=f(t,16)*dt;p.size*=f(t,30);}
            let phase=1.0-p.remaining/p.life;let head=color(t,34,phase);let tail=color(t,self.tail_curve,phase);
            let end=p.position-p.velocity*f(t,14);let direction=p.velocity.normalize_or_zero();let side=direction.cross(Vec3::Y).normalize_or_zero();let side=if side==Vec3::ZERO {Vec3::X}else{side};let other=side.cross(direction).normalize_or_zero();
            for axis in [other,side] {
                let positions=[p.position-axis*p.size,p.position+axis*p.size,end-axis*p.size/f(t,33),end+axis*p.size/f(t,33)];
                let v=if t.words[0]&0x100!=0 {1.0}else{0.0};quad(&mut out,positions,[[0.0,v],[1.0,v],[0.0,1.0-v],[1.0,1.0-v]],[0.0;4]);
                let base=out.len()-4;if p.remaining>0.0 {for (j,c) in [head,head,tail,tail].into_iter().enumerate() {out[base+j].color=c;}}
            }
        }
        if t.words[9]==0 && alive==0 {return Ok(None);}
        if t.words[9]==1 && time<self.duration-f(t,26) && f(t,12)>0.0 && self.emission>f(t,12) {
            let count=t.words[15] as usize;
            let mut emitted=0;
            for i in 0..self.particles.len() {if self.particles[i].remaining<=0.0 {self.particles[i]=self.emit(r);self.emission=0.0;emitted+=1;if emitted>=count {break;}}}
        }
        Ok(Some(vec![out]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn crystal_is_native_crossed_emitter_not_particle_or_grid_alias() {
        let mut words=vec![0;36];words[0]=515;words[9]=1;words[10]=74;words[11]=80;words[15]=3;
        for (i,v) in [(8,5.0f32),(12,0.001),(14,1.0),(17,-5.0),(18,5.0),(19,-5.0),(20,5.0),(21,-5.0),(22,5.0),(25,0.15),(26,0.4),(27,4.0),(28,8.0),(30,1.01),(33,0.5)] {words[i]=v.to_bits();}
        let t=Template {kind:3031,words};let mut rng=R250::new(0xe6f1);
        let mut effect=Crystal::new(&t,Mat4::IDENTITY,&mut rng).unwrap();
        assert!(super::super::buffs::Buff::supports(3031));
        assert_eq!(effect.models()[0].2,640);assert_eq!(effect.models()[0].1.len(),960);
        effect.vertices(0.016,&mut rng).unwrap();
        assert_eq!(effect.particles.iter().filter(|p|p.remaining>0.0).count(),3);
        let vertices=effect.vertices(0.032,&mut rng).unwrap().unwrap();
        assert!(vertices[0].iter().any(|v|v.color[3]>0.0));
        let mut truncated=t;truncated.words[34]=100;assert!(Crystal::new(&truncated,Mat4::IDENTITY,&mut rng).is_err());
    }
    #[test]
    fn crystal_graceful_deletes_without_advancing_frame_time() {
        use super::super::{Binding,Renderer};
        let dir=ao_gui::client_dir();
        if !dir.join("Setupf/gfxtweak.bin").exists() || !dir.join("cd_image/rdb.db").exists() {return;}
        let mut renderer=Renderer::open(&dir).unwrap();
        let handle=renderer.spawn_configured(Binding {group:0,attractor:0,effect:73001,note:0,color:0},Mat4::IDENTITY,Vec3::ZERO,EffectConfig::default()).unwrap();
        assert!(renderer.active.iter().any(|a|a.actor==handle));
        renderer.terminate_gracefully(handle);
        assert!(!renderer.active.iter().any(|a|a.actor==handle),"termination must not wait for time>duration");
    }
}
