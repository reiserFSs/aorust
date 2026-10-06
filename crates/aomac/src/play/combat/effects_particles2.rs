//! GC BParticle2: 1010ac44/1010ba11/1010af47; DS 1000b4fc.
//! GC BParticle: 1010a0f0/1010a3b5; DS 10009938/10008bce/1000a70f.
//! CMSGet 10106872/10106893 returns zero for absent parameters. BParticle2's
//! 36/42/44-word records have 0/3/4 colour knots at index 35, not different layouts.
//! BParticle mode 8 is the actual 71343 dependency. Other DS modes are rejected,
//! not substituted: their separate geometry/process branches remain unported.
use super::{materials, quad, sprites, EffectConfig, Template};
use anyhow::{ensure, Context, Result};
use ao_formats::{character::CrtRand, weather::R250};
use ao_scene::{Blend, Vertex};
use glam::{Mat4, Vec3};

fn word(t:&Template,i:usize)->u32 { t.words.get(i).copied().unwrap_or(0) }
fn float(t:&Template,i:usize)->f32 { f32::from_bits(word(t,i)) }
fn uniform(r:&mut R250)->f32 { super::random_fraction(r) }
fn sample(t:&Template,a:usize,b:usize,r:&mut R250)->f32 { float(t,a)+(float(t,b)-float(t,a))*uniform(r) }
fn rgba(c:u32)->[f32;4] { let [a,r,g,b]=c.to_be_bytes(); [r as f32/255.0,g as f32/255.0,b as f32/255.0,a as f32/255.0] }
fn render(mut c:[f32;4])->[f32;4] { for v in &mut c[..3] {*v=v.clamp(0.0,1.0).powf(2.2);} c[3]=c[3].clamp(0.0,1.0);c }
fn curve(t:&Template,phase:f32)->[f32;4] {
    let n=word(t,35) as usize;
    if n==0 {return [1.0;4];}
    let mut out=rgba(word(t,37+(n-1)*2));
    for i in 0..n.saturating_sub(1) {
        let a=float(t,36+i*2);let b=float(t,38+i*2);
        // Retail uses strict bounds; an exact knot falls back to the last colour.
        if a<phase && phase<b {let c=rgba(word(t,37+i*2));let d=rgba(word(t,39+i*2));let u=(phase-a)/if b==a {1.0}else{b-a};out=std::array::from_fn(|j|c[j]+(d[j]-c[j])*u);break;}
    }
    out
}
struct Particle { position:Vec3, velocity:Vec3, remaining:f32, life:f32, size:f32, angle:f32, spin:f32, frame:f32, phase:f32, delay:f32 }
pub(super) struct ParticleEffect { template:Template, source:Mat4, particles:Vec<Particle>, previous:f32, emission:f32, frame_rate:f32, duration:f32 }
impl ParticleEffect {
    pub(super) fn supports(kind:i32)->bool {matches!(kind,3024|3028)}
    pub(super) fn new(t:&Template,source:Mat4,_target:Mat4,_color:u32,gc:&mut R250,ds:&mut R250,_crt:&mut CrtRand)->Result<Self> {
        ensure!(Self::supports(t.kind),"unsupported BParticle class {}",t.kind);
        t.word(if t.kind==3028 {34}else{43})?;
        if t.kind==3024 {ensure!(word(t,10)==8,"unported native BParticle mode {} (DS10009938)",word(t,10));ensure!(word(t,13)==0 && word(t,14)==0 && word(t,15)==0,"unported BParticle motion/size/geometry mode");}
        else {ensure!(word(t,9)<=1 && word(t,32)<=3 && word(t,34)==0,"unported BParticle2 emission/atlas/geometry mode");ensure!(word(t,0)&(0x400|0x8000|0x10000|0x400000|0x800000|0x1000000)==0,"BParticle2 requires native terrain/environment/body-scale input");}
        let integers=if t.kind==3028 {vec![0,7,9,10,11,15,32,34,35]}else{vec![0,7,9,10,11,13,14,15,25,26,27,39]};
        let end=if t.kind==3028 {35}else{44};
        for i in 0..end {if !integers.contains(&i) {ensure!(float(t,i).is_finite(),"nonfinite BParticle parameter {i}");}}
        if t.kind==3028 {let n=word(t,35) as usize;ensure!(n<=t.words.len().saturating_sub(36)/2,"truncated BParticle2 colour curve");for i in 0..n {ensure!(float(t,36+i*2).is_finite(),"nonfinite BParticle2 knot");}ensure!(float(t,25)>0.0 && float(t,26)>0.0 && float(t,33)>0.0,"invalid BParticle2 lifetime/aspect");}
        let material=word(t,if t.kind==3028 {10}else{9}) as usize;
        materials::MATERIALS.get(material).context("unknown BParticle material")?;
        let count=word(t,11) as usize;ensure!(count<=u16::MAX as usize/4,"BParticle capacity exceeds native indices");
        let mut out=Self {template:t.clone(),source:sprites::connector(t,source)?,particles:Vec::with_capacity(count),previous:0.0,emission:0.0,frame_rate:float(t,31),duration:float(t,8)};
        for i in 0..count {
            let p=if t.kind==3028 {out.emit(gc)}else{
                // DS mode 8 consumes only its shared DisplaySystem R250 stream.
                let position=Vec3::new(uniform(ds)*2.0-1.0,uniform(ds)*2.0-1.0,-(uniform(ds)*2.0-1.0))*float(t,12);
                let delay=uniform(ds)*0.2;
                Particle {position,velocity:Vec3::ZERO,remaining:1.0,life:1.0,size:1.0,angle:0.0,spin:0.0,frame:0.0,phase:999.0,delay:if i==0 {0.0}else{delay}}
            };out.particles.push(p);
        }
        Ok(out)
    }
    fn emit(&self,r:&mut R250)->Particle {
        let t=&self.template;let size=sample(t,27,28,r);let life=sample(t,25,26,r);
        let material=&materials::MATERIALS[word(t,10) as usize];
        let frame=if word(t,0)&0x20000!=0 {material.3 as f32+(material.4-material.3) as f32*uniform(r)}else{0.0};
        let position=self.source.w_axis.truncate()+Vec3::new(uniform(r)*2.0-1.0,uniform(r)*2.0-1.0,-(uniform(r)*2.0-1.0))*float(t,13);
        let velocity=self.source.transform_vector3(Vec3::new(sample(t,17,18,r),sample(t,19,20,r),-sample(t,21,22,r)));
        let angle=if word(t,0)&0x4000==0 {uniform(r)*std::f32::consts::TAU}else{0.0};
        let spin=sample(t,23,24,r).to_radians();
        Particle {position,velocity,remaining:life,life,size,angle,spin,frame,phase:0.0,delay:0.0}
    }
    pub(super) fn configure(&mut self,c:EffectConfig)->Result<()> {if let Some(d)=c.duration {self.duration=d;}Ok(())}
    pub(super) fn update_source(&mut self,source:Mat4)->Result<()> {self.source=sprites::connector(&self.template,source)?;Ok(())}
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {let n=self.particles.len();vec![(Some(word(&self.template,if self.template.kind==3028 {10}else{9}) as usize),(0..n as u32).flat_map(|i|[i*4,i*4+2,i*4+3,i*4,i*4+3,i*4+1]).collect(),n*4)]}
    pub(super) fn blends(&self)->Vec<Blend> {vec![if word(&self.template,0)&0x200!=0 {Blend::Additive}else{Blend::AlphaBlend}]}
    // Preserve the native frame inputs and separate Gamecode/DisplaySystem/CRT RNG streams.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn vertices(&mut self,time:f32,_camera:Vec3,right:Vec3,up:Vec3,gc:&mut R250,ds:&mut R250,_crt:&mut CrtRand)->Result<Option<Vec<Vec<Vertex>>>> {
        let dt=(time-self.previous).max(0.0);self.previous=time;self.emission+=dt;
        let t=&self.template;let modern=t.kind==3028;
        if !modern && self.duration>=0.0 && time>self.duration {return Ok(None);}
        let material=materials::MATERIALS[word(t,if modern {10}else{9}) as usize];
        let mut alive=0;let mut vertices=Vec::with_capacity(self.particles.len()*4);
        for p in &mut self.particles {
            if modern {
                if word(t,0)&0x200000==0 {p.remaining-=dt;}
                if p.remaining>0.0 {
                    alive+=1;
                    if word(t,0)&0x100000==0 {p.position+=p.velocity*dt;p.velocity+=self.source.transform_vector3(Vec3::Y*float(t,16))*dt;}else{p.position=self.source.w_axis.truncate();}
                    p.angle+=p.spin*dt;p.size+=dt*p.size*(float(t,30)-1.0)*100.0;
                    p.velocity+=p.velocity*(float(t,14)-1.0)*dt*100.0;
                    if word(t,32)!=0 {
                        p.frame+=self.frame_rate*dt;
                        if p.frame>material.4 as f32 {match word(t,32) {1=>p.frame=material.3 as f32,2=>p.frame=material.4 as f32,3=>{self.frame_rate= -self.frame_rate;p.frame+=self.frame_rate*dt;},_=>{}}}
                        if p.frame<material.3 as f32 && word(t,32)==3 {self.frame_rate= -self.frame_rate;p.frame+=self.frame_rate*dt;}
                    }
                }
            } else {
                if p.phase==999.0 || p.delay==999.0 {p.phase-=float(t,32)*dt;}else {p.delay-=float(t,31)*dt;if p.delay<0.0 {p.phase=0.5+uniform(ds)*0.5;p.delay=999.0;p.angle=uniform(ds)*360.0;p.position=Vec3::new(uniform(ds)*2.0-1.0,uniform(ds)*2.0-1.0,-(uniform(ds)*2.0-1.0))*float(t,12);}}
            }
            let visible=if modern {p.remaining>0.0}else{p.phase>0.0 && p.phase!=999.0};
            let mut color=if modern {curve(t,1.0-p.remaining/p.life)}else{rgba(word(t,26))};
            let pulse=(p.phase*std::f32::consts::FRAC_PI_2).sin();
            if !modern {color[3]*=pulse;}
            if !visible {color=[0.0;4];}
            let size=if modern {p.size}else{float(t,33)*pulse};
            let height=if modern {size/float(t,33)}else{float(t,34)*pulse};
            let angle=if modern {-p.angle}else{-p.angle.to_radians()};let x=(right*angle.cos()+up*angle.sin())*size;let y=(-right*angle.sin()+up*angle.cos())*height;
            let pos=if modern {p.position}else{self.source.transform_point3(p.position)};
            let frame=p.frame as i32;let u=frame.rem_euclid(material.1 as i32) as f32/material.1 as f32;let v=(frame/material.1 as i32) as f32/material.2 as f32;let du=1.0/material.1 as f32;let dv=1.0/material.2 as f32;
            quad(&mut vertices,[pos-x-y,pos+x-y,pos-x+y,pos+x+y],[[u,v+dv],[u+du,v+dv],[u,v],[u+du,v]],render(color));
        }
        if modern {
            if word(t,9)==0 && alive==0 {return Ok(None);}
            if word(t,9)==1 && (self.duration<0.0 || time<self.duration-float(t,26)) && float(t,12)>0.0 && self.emission>float(t,12) {
                let mut emitted=0;for i in 0..self.particles.len() {if self.particles[i].remaining<=0.0 {self.emission=0.0;self.particles[i]=self.emit(gc);emitted+=1;if emitted>=word(t,15) {break;}}}
            }
            if self.duration>=0.0 && time>self.duration {return Ok(None);}
        }
        Ok(Some(vec![vertices]))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn native_cms_lengths_and_strict_color_knots() {
        let mut t=Template {kind:3028,words:vec![0;36]};assert_eq!(word(&t,99),0);assert_eq!(curve(&t,0.5),[1.0;4]);
        t.words.resize(42,0);t.words[35]=3;t.words[38]=0.5f32.to_bits();t.words[40]=1.0f32.to_bits();t.words[37]=0x00ffffff;t.words[39]=0xffffffff;t.words[41]=0x00ffffff;
        assert_eq!(curve(&t,0.5)[3],0.0);assert!((curve(&t,0.25)[3]-0.5).abs()<1e-6);
        t.words.resize(44,0);t.words[35]=4;t.words[42]=1.5f32.to_bits();assert_eq!(curve(&t,1.5),rgba(0));
    }
    #[test]
    #[ignore = "requires installed retail gfxtweak and offscreen GPU rendering"]
    fn retail_particle_dependency_frames()->Result<()> {
        use super::super::{Binding, Renderer, MODEL_BASE};
        let templates=super::super::Templates::open(&ao_gui::client_dir())?;
        let mut gc=R250::new(0xe6f1);let mut ds=R250::new(0xe6f1);let mut crt=CrtRand::new(1);
        for id in [71516,71904,71905,71906,71340,71341,71343] {
            let t=templates.by_id.get(&id).with_context(||format!("missing retail particle {id}"))?;
            let mut effect=ParticleEffect::new(t,Mat4::IDENTITY,Mat4::IDENTITY,0,&mut gc,&mut ds,&mut crt)?;
            let mut visible=false;
            for step in 1..=20 {
                if let Some(groups)=effect.vertices(step as f32*0.01,Vec3::Z*10.0,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt)? {
                    for v in groups.iter().flatten() {ensure!(v.pos.iter().chain(v.color.iter()).all(|x|x.is_finite()),"nonfinite particle {id}");visible|=v.color[3]>0.0;}
                }
            }
            ensure!(visible,"retail particle {id} emitted no visible frame");
        }
        let out = std::env::var_os("AOMAC_EFFECT_FRAMES")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| "/tmp/FxRest/frames".into());
        std::fs::create_dir_all(&out)?;
        let mut renderer = Renderer::open(&ao_gui::client_dir())?;
        let mut host = ao_render::Host::headless();
        for id in [71516,71904,71905,71906,71340,71341,71343] {
            renderer.clear();
            host.camera = ao_render::Camera::look_at(Vec3::new(2.0,2.0,5.0), Vec3::ZERO);
            renderer.spawn(Binding { group:0, attractor:0, effect:id, note:0, color:0 },
                Mat4::IDENTITY, Vec3::X)?;
            let mut rendered = false;
            for step in 1..=81 {
                host.actors.clear();
                renderer.frame(0.01, &mut host, None);
                let vertices: Vec<_> = host.actors.iter().filter_map(|a| a.skin.as_ref())
                    .flatten().filter(|v| v.color[3]>0.0).collect();
                if vertices.is_empty() || (rendered && ![11,21,51,81].contains(&step)) { continue; }
                let mut min = Vec3::splat(f32::INFINITY);
                let mut max = Vec3::splat(f32::NEG_INFINITY);
                for v in vertices {
                    ensure!(v.pos.iter().chain(v.color.iter()).all(|x| x.is_finite()),
                        "nonfinite rendered particle {id}");
                    let p = Vec3::from_array(v.pos);
                    min = min.min(p);
                    max = max.max(p);
                }
                // Frame the actual native geometry, without changing its size or colours.
                let center = (min+max)*0.5;
                let eye = center + Vec3::new(2.0,2.0,5.0).normalize()
                    * ((max-min).length()*1.5).max(0.1);
                let models: Vec<_> = renderer.models.iter()
                    .map(|(&id,m)| (MODEL_BASE|id as u32 as u64,m.scene.clone())).collect();
                let time = (step-1) as f32*0.01;
                let path = out.join(format!("particle3028_3024_{id}_{step}.png"));
                let background = out.join(format!("particle_background_{id}_{step}.png"));
                ao_render::render_to_png_actors(&ao_scene::Scene::default(), &models,
                    host.actors.clone(), eye.to_array(), center.to_array(), 640,480, &path,time)?;
                ao_render::render_to_png_actors(&ao_scene::Scene::default(), &[], Vec::new(),
                    eye.to_array(), center.to_array(), 640,480, &background,time)?;
                let frame = image::open(&path)?.to_rgba8();
                let blank = image::open(&background)?.to_rgba8();
                let pixels = frame.pixels().zip(blank.pixels()).filter(|(a,b)| a!=b).count();
                eprintln!("particle {id} frame {step} at {time:.2}s: {pixels} visible resource pixels, {}", path.display());
                // Native atlas frame zero can be black under additive blending;
                // 71341's retail alpha knots also stay zero until phase .15.
                // Require actual GPU visibility over the authored timeline, not at birth.
                rendered |= pixels > 0;
            }
            ensure!(rendered, "retail particle {id} produced no GPU frame");
        }
        Ok(())
    }
}
