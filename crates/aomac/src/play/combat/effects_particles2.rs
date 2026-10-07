//! GC BParticle2: 1010ac44/1010ba11/1010af47; DS 1000b4fc.
//! CMSGet 10106872/10106893 returns zero for absent parameters. BParticle2's
//! 36/42/44-word records have 0/3/4 colour knots at index 35, not different layouts.
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
struct Particle { position:Vec3, velocity:Vec3, acceleration:Vec3, remaining:f32, life:f32, size:f32, angle:f32, spin:f32, frame:f32 }
pub(super) struct ParticleEffect { template:Template, source:Mat4, particles:Vec<Particle>, previous:f32, emission:f32, frame_rate:f32, duration:f32, terminated:bool,body_scale:f32,environment_time:Option<f32> }
impl ParticleEffect {
    pub(super) fn supports(kind:i32)->bool {kind==3028}
    pub(super) fn new(t:&Template,source:Mat4,_target:Mat4,_color:u32,gc:&mut R250,_ds:&mut R250,_crt:&mut CrtRand)->Result<Self> {
        ensure!(Self::supports(t.kind),"unsupported BParticle class {}",t.kind);
        let integers=[0,7,9,10,11,15,32,34,35];
        let end=35;
        for i in 0..end {if !integers.contains(&i) {ensure!(float(t,i).is_finite(),"nonfinite BParticle parameter {i}");}}
        let n=word(t,35) as usize;ensure!(n<=t.words.len().saturating_sub(36)/2,"truncated BParticle2 colour curve");for i in 0..n {ensure!(float(t,36+i*2).is_finite(),"nonfinite BParticle2 knot");}ensure!(float(t,25)>0.0 && float(t,26)>0.0 && float(t,33)>0.0,"invalid BParticle2 lifetime/aspect");
        let material=word(t,10) as usize;
        materials::MATERIALS.get(material).context("unknown BParticle material")?;
        let count=word(t,11) as usize;ensure!(count<=u16::MAX as usize/4,"BParticle capacity exceeds native indices");
        let mut out=Self {template:t.clone(),source:sprites::connector(t,source)?,particles:Vec::with_capacity(count),previous:0.0,emission:0.0,frame_rate:float(t,31),duration:float(t,8),terminated:false,body_scale:1.0,environment_time:None};
        for _ in 0..count {let mut p=out.emit(gc);if word(t,9)==1 {p.remaining=0.0;}out.particles.push(p);}
        Ok(out)
    }
    fn emit(&self,r:&mut R250)->Particle {
        let t=&self.template;let size=sample(t,27,28,r);let life=sample(t,25,26,r);
        let material=&materials::MATERIALS[word(t,10) as usize];
        let frame=if word(t,0)&0x20000!=0 {material.3 as f32+(material.4-material.3) as f32*uniform(r)}else{0.0};
        let position=self.source.w_axis.truncate()+Vec3::new(uniform(r)*2.0-1.0,uniform(r)*2.0-1.0,-(uniform(r)*2.0-1.0))*float(t,13);
        let scale=if word(t,0)&0x1000000!=0 {self.body_scale*self.body_scale}else{self.body_scale};
        let velocity=self.source.transform_vector3(Vec3::new(sample(t,17,18,r),sample(t,19,20,r),-sample(t,21,22,r)))*scale;
        let angle=if word(t,0)&0x4000==0 {uniform(r)*std::f32::consts::TAU}else{0.0};
        let spin=sample(t,23,24,r).to_radians();
        let acceleration=self.source.transform_vector3(Vec3::Y*float(t,16))*scale;
        Particle {position,velocity,acceleration,remaining:life,life,size,angle,spin,frame}
    }
    pub(super) fn configure(&mut self,c:EffectConfig)->Result<()> {if let Some(d)=c.duration {self.duration=d;}Ok(())}
    pub(super) fn set_native_inputs(&mut self,body_scale:f32,environment_time:Option<f32>) {if word(&self.template,0)&0x1000000!=0 {self.body_scale=body_scale;}self.environment_time=environment_time;}
    // Disabled visuals freeze particle/emission clocks, while base control time keeps advancing.
    pub(super) fn set_visible(&mut self,visible:bool,time:f32) {if !visible {self.previous=time;}}
    // GC1010aa16 sets +28. GC1010af47 immediately ends ordinary particles;
    // the 0x200000 held-life branch instead releases their remaining lifetimes.
    pub(super) fn terminate_gracefully(&mut self) {self.terminated=true;}
    pub(super) fn update_source(&mut self,source:Mat4)->Result<()> {self.source=sprites::connector(&self.template,source)?;Ok(())}
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {let n=self.particles.len();vec![(Some(word(&self.template,10) as usize),(0..n as u32).flat_map(|i|[i*4,i*4+1,i*4+2,i*4+2,i*4+1,i*4+3]).collect(),n*4)]}
    pub(super) fn blends(&self)->Vec<Blend> {vec![if word(&self.template,0)&0x200!=0 {Blend::Additive}else{Blend::AlphaBlend}]}
    // Preserve the native frame inputs and separate Gamecode/DisplaySystem/CRT RNG streams.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn vertices(&mut self,time:f32,_camera:Vec3,right:Vec3,up:Vec3,gc:&mut R250,_ds:&mut R250,_crt:&mut CrtRand,mut terrain:Option<&mut dyn FnMut(Vec3)->Option<(Vec3,Vec3)>>)->Result<Option<Vec<Vec<Vertex>>>> {
        let initial=self.previous==0.0;let dt=(time-self.previous).max(0.0);self.previous=time;self.emission+=dt;
        let source_height=if word(&self.template,0)&0x800000!=0 {
            let query=terrain.as_mut().context("BParticle2 height fade requires terrain")?;
            self.source.w_axis.y-query(self.source.w_axis.truncate()).context("missing BParticle2 source ground")?.0.y
        }else{0.0};
        if word(&self.template,0)&0x400!=0 {
            let query=terrain.as_mut().context("BParticle2 ground snapping requires terrain")?;
            let old_y=self.source.w_axis.y;
            self.source.w_axis.y=query(self.source.w_axis.truncate()).context("missing BParticle2 source ground")?.0.y;
            if initial {for p in &mut self.particles {p.position.y+=self.source.w_axis.y-old_y;}}
        }
        let t=&self.template;
        if self.terminated && word(t,0)&0x200000==0 {return Ok(None);}
        let material=materials::MATERIALS[word(t,10) as usize];
        let mut alive=0;let mut vertices=Vec::with_capacity(self.particles.len()*4);
        for p in &mut self.particles {
                if word(t,0)&0x200000==0 || self.terminated {p.remaining-=dt;}
                if p.remaining>0.0 {
                    alive+=1;
                    if word(t,0)&0x8000!=0 {
                        let query=terrain.as_mut().context("BParticle2 bounce requires terrain")?;
                        if p.position.y<=query(p.position).context("missing BParticle2 particle ground")?.0.y {
                            p.velocity*=float(t,29);p.velocity.y= -p.velocity.y;p.spin=sample(t,23,24,gc).to_radians();
                        }
                    }
                    if word(t,0)&0x100000==0 {p.position+=p.velocity*dt;p.velocity+=p.acceleration*dt;}else{p.position=self.source.w_axis.truncate();}
                    p.angle+=p.spin*dt;p.size+=dt*p.size*(float(t,30)-1.0)*100.0;
                    p.velocity+=p.velocity*(float(t,14)-1.0)*dt*100.0;
                    if word(t,32)!=0 {
                        p.frame+=self.frame_rate*dt;
                        if p.frame>material.4 as f32 {match word(t,32) {1=>p.frame=material.3 as f32,2=>p.frame=material.4 as f32,3=>{self.frame_rate= -self.frame_rate;p.frame+=self.frame_rate*dt;},_=>{}}}
                        if p.frame<material.3 as f32 && word(t,32)==3 {self.frame_rate= -self.frame_rate;p.frame+=self.frame_rate*dt;}
                    }
                }
            let visible=p.remaining>0.0;
            let mut color=curve(t,1.0-p.remaining/p.life);
            if word(t,0)&0x800000!=0 {let factor=if source_height>6.0 {0.0}else if source_height>3.0 && source_height<6.0 {1.0-(source_height-3.0)/3.0}else{1.0};for c in &mut color {*c=(*c*factor*255.0) as u8 as f32/255.0;}}
            if word(t,0)&0x400000!=0 {
                let mut factor=self.environment_time.context("BParticle2 environment flag requires native day time")?/3240.0;
                if factor>1.0 {factor=2.0-factor;}factor=factor.max(0.3);
                for c in &mut color {*c=(*c*factor*255.0) as u8 as f32/255.0;}
            }
            if !visible {color=[0.0;4];}
            let size=p.size;let height=size/float(t,33);
            let angle=-p.angle;let x=(right*angle.cos()+up*angle.sin())*size;let y=(-right*angle.sin()+up*angle.cos())*height;
            let pos=p.position;
            let frame=p.frame as i32;let u=frame.rem_euclid(material.1 as i32) as f32/material.1 as f32;let v=(frame/material.1 as i32) as f32/material.2 as f32;let du=1.0/material.1 as f32;let dv=1.0/material.2 as f32;
            // DS1000b4fc keeps four vertices even for its triangular mode.
            let positions=match word(t,34) {0=>[pos+x+y,pos+y-x,pos+x-y,pos-x-y],1=>[pos+x+y,pos+y,pos+x,pos],2=>[pos+2.0*(x+y),pos+2.0*(y-x),pos+x,pos-x],_=>[pos;4]};
            let uv=if word(t,0)&0x100!=0 {[[u,v],[u,v+dv],[u+du,v],[u+du,v+dv]]}else{[[u,v],[u+du,v],[u,v+dv],[u+du,v+dv]]};
            quad(&mut vertices,positions,uv,render(color));
        }
            if (word(t,9)==0 || self.terminated) && alive==0 {return Ok(None);}
            // GC1010af47 accepts zero intervals too: emit once per positive-dt process.
            if !self.terminated && word(t,9)==1 && (self.duration<0.0 || time<self.duration-float(t,26)) && float(t,12)>=0.0 && self.emission>float(t,12) {
                let mut emitted=0;for i in 0..self.particles.len() {if self.particles[i].remaining<=0.0 {
                    self.emission=0.0;let mut p=self.emit(gc);
                    if word(&self.template,0)&0x10000!=0 {let query=terrain.as_mut().context("BParticle2 ground emission requires terrain")?;p.position.y=query(p.position).context("missing BParticle2 emission ground")?.0.y+float(&self.template,2);}
                    self.particles[i]=p;emitted+=1;if emitted>=word(&self.template,15) {break;}
                }}
            }
            if self.duration>=0.0 && time>self.duration {return Ok(None);}
        Ok(Some(vec![vertices]))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authored_bparticle2_modes_preserve_display_and_crt_streams()->Result<()> {
        let templates=super::super::Templates::open(&ao_gui::client_dir())?;
        for mode in 0..=2 {
            let mut t=templates.by_id[&71516].clone();t.words[34]=mode;t.words[0]|=0x400|0x8000|0x10000|0x400000|0x800000|0x1000000;
            let mut gc=R250::new(0xe6f1);let mut ds=R250::new(77);let mut crt=CrtRand::new(91);
            let mut expected_ds=ds.clone();let mut expected_crt=CrtRand::new(91);
            let mut effect=ParticleEffect::new(&t,Mat4::IDENTITY,Mat4::IDENTITY,0,&mut gc,&mut ds,&mut crt)?;
            effect.set_native_inputs(1.5,Some(3240.0));
            let mut terrain=|p:Vec3|Some((Vec3::new(p.x,0.0,p.z),Vec3::Y));
            let vertices=effect.vertices(0.01,Vec3::Z,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt,Some(&mut terrain))?;
            assert!(vertices.is_some());assert_eq!(ds.next_u32(),expected_ds.next_u32());assert_eq!(crt.rand(),expected_crt.rand());
        }Ok(())
    }
    #[test] fn native_cms_lengths_and_strict_color_knots() {
        let mut t=Template {kind:3028,words:vec![0;36]};assert_eq!(word(&t,99),0);assert_eq!(curve(&t,0.5),[1.0;4]);
        t.words.resize(42,0);t.words[35]=3;t.words[38]=0.5f32.to_bits();t.words[40]=1.0f32.to_bits();t.words[37]=0x00ffffff;t.words[39]=0xffffffff;t.words[41]=0x00ffffff;
        assert_eq!(curve(&t,0.5)[3],0.0);assert!((curve(&t,0.25)[3]-0.5).abs()<1e-6);
        t.words.resize(44,0);t.words[35]=4;t.words[42]=1.5f32.to_bits();assert_eq!(curve(&t,1.5),rgba(0));
    }
    #[test]
    fn ordinary_bparticle2_graceful_termination_ends_rendering() {
        let mut effect = ParticleEffect {template:Template {kind:3028,words:vec![0;36]},source:Mat4::IDENTITY,particles:vec![],previous:0.0,emission:0.0,frame_rate:0.0,duration:-1.0,terminated:false,body_scale:1.0,environment_time:None};
        effect.terminate_gracefully();
        let mut gc=R250::new(1);let mut ds=R250::new(1);let mut crt=CrtRand::new(1);
        assert!(effect.vertices(0.0,Vec3::Z,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt,None).unwrap().is_none());
    }
    #[test]
    fn invisible_native_particles_freeze_simulation_without_catchup() {
        let p=Particle {position:Vec3::Y,velocity:Vec3::X,acceleration:Vec3::ZERO,remaining:2.0,life:2.0,size:1.0,angle:0.0,spin:0.0,frame:0.0};
        let mut effect=ParticleEffect {template:Template {kind:3028,words:vec![0;36]},source:Mat4::IDENTITY,particles:vec![p],previous:1.0,emission:0.25,frame_rate:0.0,duration:-1.0,terminated:false,body_scale:1.0,environment_time:None};
        effect.set_visible(false,10.0);
        assert_eq!(effect.previous,10.0);assert_eq!(effect.emission,0.25);
        assert_eq!(effect.particles[0].remaining,2.0);assert_eq!(effect.particles[0].position,Vec3::Y);
        effect.set_visible(true,10.01);
        assert_eq!(effect.previous,10.0);assert_eq!(effect.emission,0.25);assert_eq!(effect.particles[0].remaining,2.0);
    }
    #[test]
    fn authored_zero_interval_recycles_once_per_process()->Result<()> {
        let templates=super::super::Templates::open(&ao_gui::client_dir())?;
        let t=&templates.by_id[&72219];
        assert_eq!(word(t,9),1);assert_eq!(float(t,12),0.0);
        let mut gc=R250::new(0xe6f1);let mut ds=R250::new(0xe6f1);let mut crt=CrtRand::new(1);
        let mut effect=ParticleEffect::new(t,Mat4::IDENTITY,Mat4::IDENTITY,0,&mut gc,&mut ds,&mut crt)?;
        assert!(effect.particles.iter().all(|p|p.remaining==0.0));
        effect.vertices(0.01,Vec3::Z,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt,None)?;
        assert_eq!(effect.particles.iter().filter(|p|p.remaining>0.0).count(),word(t,15) as usize);
        effect.vertices(0.02,Vec3::Z,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt,None)?;
        assert_eq!(effect.particles.iter().filter(|p|p.remaining>0.0).count(),2*word(t,15) as usize);
        Ok(())
    }
    #[test]
    fn authored_held_life_colour_curve_waits_for_release()->Result<()> {
        let templates=super::super::Templates::open(&ao_gui::client_dir())?;
        let t=&templates.by_id[&72414];
        assert_ne!(word(t,0)&0x200000,0);
        let mut gc=R250::new(0xe6f1);let mut ds=R250::new(0xe6f1);let mut crt=CrtRand::new(1);
        let mut effect=ParticleEffect::new(t,Mat4::IDENTITY,Mat4::IDENTITY,0,&mut gc,&mut ds,&mut crt)?;
        let held=effect.vertices(0.01,Vec3::Z,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt,None)?.unwrap();
        assert!(held.iter().flatten().all(|v|v.color[3]==0.0));
        effect.terminate_gracefully();
        let released=effect.vertices(0.02,Vec3::Z,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt,None)?.unwrap();
        assert!(released.iter().flatten().any(|v|v.color[3]>0.0));
        Ok(())
    }



    #[test]
    #[ignore = "requires installed retail gfxtweak and offscreen GPU rendering"]
    fn retail_particle_dependency_frames()->Result<()> {
        use super::super::{Binding, Renderer, MODEL_BASE};
        let templates=super::super::Templates::open(&ao_gui::client_dir())?;
        let mut gc=R250::new(0xe6f1);let mut ds=R250::new(0xe6f1);let mut crt=CrtRand::new(1);
        let ids:Vec<_>=templates.by_id.iter().filter_map(|(&id,t)|(t.kind==3028).then_some(id)).collect();
        for &id in &ids {
            let t=templates.by_id.get(&id).with_context(||format!("missing retail particle {id}"))?;
            let mut effect=ParticleEffect::new(t,Mat4::IDENTITY,Mat4::IDENTITY,0,&mut gc,&mut ds,&mut crt)?;
            effect.set_native_inputs(1.0,Some(ao_formats::playfield::DEFAULT_DAY_TIME));
            let mut visible=false;
            let steps=((float(t,25).max(float(t,26))+float(t,12).max(0.0)+0.02)*100.0).ceil() as usize;
            for step in 1..=steps {
                // Held-life particles only advance their authored colour curve after
                // native graceful release (GC1010aa16 / GC1010af47).
                if step==2 && word(t,0)&0x200000!=0 {effect.terminate_gracefully();}
                let mut terrain=|p:Vec3|Some((Vec3::new(p.x,0.0,p.z),Vec3::Y));
                if let Some(groups)=effect.vertices(step as f32*0.01,Vec3::Z*10.0,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt,Some(&mut terrain))? {
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
        for &id in &ids {
            renderer.clear();
            host.camera = ao_render::Camera::look_at(Vec3::new(2.0,2.0,5.0), Vec3::ZERO);
            let handle=renderer.spawn_configured(Binding { group:0, attractor:0, effect:id, note:0, color:0 },
                Mat4::IDENTITY, Vec3::X, super::super::EffectConfig {creation:super::super::Creation::Vector,..Default::default()})?;
            let mut rendered = false;
            let t=&templates.by_id[&id];
            let steps=((float(t,25).max(float(t,26))+float(t,12).max(0.0)+0.02)*100.0).ceil() as usize;
            for step in 1..=steps {
                host.actors.clear();
                if step==2 && word(&templates.by_id[&id],0)&0x200000!=0 {renderer.terminate_gracefully(handle);}
                let mut terrain=|p:Vec3|Some((Vec3::new(p.x,0.0,p.z),Vec3::Y));
                renderer.frame(0.01, &mut host, Some(&mut terrain));
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
