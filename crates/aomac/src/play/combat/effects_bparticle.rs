//! BParticle3024: GC1010a0f0/10109ebe/1010a3b5; DS10009938/10008bce/1000a70f.
//! Particle fields follow the native 0x58-byte layout; DisplaySystem owns every draw.
//! Installed gfxtweak: 33 records, initializer selectors0,1,2,3,6,7,8,9,11,13.
//! Record72340 selects13: DS10009938's memset precedes the0–12 switch,
//! so it retains zero life and DS1000a70f emits nothing (artifact22600).
use super::{materials, quad, sprites, EffectConfig, Template};
use anyhow::{ensure, Context, Result};
use ao_formats::{character::CrtRand, weather::R250};
use ao_scene::{Blend, Vertex};
use glam::{Mat4, Vec3};
fn w(t:&Template,i:usize)->u32 {t.words.get(i).copied().unwrap_or(0)}
fn f(t:&Template,i:usize)->f32 {f32::from_bits(w(t,i))}
fn r(rng:&mut R250)->f32 {super::random_fraction(rng)}
fn sample(t:&Template,a:usize,b:usize,rng:&mut R250)->f32 {f(t,a)+(f(t,b)-f(t,a))*r(rng)}
fn position(t:&Template,rng:&mut R250)->Vec3 {Vec3::new(r(rng)*2.0-1.0,r(rng)*2.0-1.0,-(r(rng)*2.0-1.0))*f(t,12)}
fn rgba(c:u32)->[f32;4] {let [a,r,g,b]=c.to_be_bytes();[r as f32/255.0,g as f32/255.0,b as f32/255.0,a as f32/255.0]}
#[derive(Default)]
struct Particle {position:Vec3,velocity:Vec3,acceleration:Vec3,angle:f32,spin:f32,frame:f32,life:f32,phase:f32,size:f32,delay:f32,uv:Vec3,origin:Vec3}
pub(super) struct ParticleEffect {template:Template,source:Mat4,particles:Vec<Particle>,previous:f32,emission:f32,frame_rate:f32,duration:f32}
impl ParticleEffect {
    pub(super) fn supports(kind:i32)->bool {kind==3024}
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(t:&Template,source:Mat4,_target:Mat4,_color:u32,_gc:&mut R250,ds:&mut R250,_crt:&mut CrtRand)->Result<Self> {
        ensure!(Self::supports(t.kind),"unsupported BParticle class {}",t.kind);
        for i in 0..t.words.len().min(44) {if ![0,7,9,10,11,13,14,15,25,26,27,39].contains(&i) {ensure!(f(t,i).is_finite(),"nonfinite BParticle field {i}");}}
        ensure!(w(t,13)<=4 && w(t,14)<=2 && w(t,15)<=5 && w(t,39)<=3,"invalid BParticle native mode");
        materials::MATERIALS.get(w(t,9) as usize).context("unknown BParticle material")?;
        let count=w(t,11) as usize;ensure!(count>0 && count<=u16::MAX as usize/4,"invalid BParticle capacity");
        let mut out=Self {template:t.clone(),source:sprites::connector(t,source)?,particles:Vec::with_capacity(count),previous:0.0,emission:0.0,frame_rate:f(t,38),duration:f(t,8)};
        // GC refreshes and grounds the locator again before the first visual draw.
        for i in 0..count {
            let mut p=Particle::default();
            match w(t,10) {
                0|3=>{p.position=position(t,ds);p.phase=r(ds)*360.0;p.size=r(ds)*2.0;p.origin=p.position;p.angle=r(ds)*360.0;p.spin=(r(ds)*2.0-1.0)*f(t,16);p.life=1.0;}
                1=>{p.angle=r(ds)*360.0;p.life=1.0;p.size=r(ds)*2.0-1.0;p.delay=r(ds);}
                2=>{p.angle=r(ds)*360.0;p.spin=(r(ds)*2.0-1.0)*f(t,16);p.life=1.0;}
                4=>{p.position.y=i as f32*f(t,12)/count as f32;p.phase=1.0-i as f32/(2.0*count as f32);p.angle=r(ds)*360.0;p.life=1.0;}
                5|12=>{Self::reset(t,&mut p,ds,false);}
                6=>{p.size=r(ds)*2.0-1.0;p.angle=r(ds)*360.0;p.spin=(r(ds)*2.0-1.0)*f(t,16);p.life=1.0;}
                7=>{p.position=position(t,ds);p.phase=r(ds)*360.0;p.size=r(ds)*2.0-1.0;p.uv=Vec3::new(r(ds),r(ds),r(ds));p.origin=p.position;p.angle=r(ds)*360.0;p.spin=(r(ds)*2.0-1.0)*f(t,16);p.life=1.0;}
                8=>{p.position=position(t,ds);p.life=1.0;p.size=999.0;p.delay=r(ds)*0.2;if i==0 {p.delay=0.0;}}
                9=>{p.phase=1.0;p.angle=r(ds)*360.0;p.spin=(r(ds)*2.0-1.0)*f(t,16);p.life=1.0;}
                10=>{p.phase=r(ds);p.angle=r(ds)*360.0;p.spin=(r(ds)*2.0-1.0)*f(t,16);p.life=1.0;}
                11=>{p.phase=1.0;let m=materials::MATERIALS[w(t,9) as usize];p.frame=m.3 as f32+(m.4-m.3) as f32*r(ds);p.angle=r(ds)*360.0;p.spin=(r(ds)*2.0-1.0)*f(t,16);p.life=1.0;}
                // DS10009938 zeroes the entire particle array before its 0–12
                // switch. Other selectors (authored72340 uses13) keep those zeros.
                _=>{},
            }
            if w(t,10)==12 {p.life=if i==0 {f(t,43)}else{0.0};}
            out.particles.push(p);
        }
        Ok(out)
    }
    fn ground(&mut self,terrain:Option<&mut dyn FnMut(Vec3)->Option<(Vec3,Vec3)>>)->Result<()> {
        if w(&self.template,0)&0x400!=0 {let query=terrain.context("BParticle ground flag requires terrain")?;let p=self.source.w_axis.truncate();let (hit,_)=query(p).context("BParticle source has no ground")?;self.source.w_axis.y=hit.y;}Ok(())
    }
    fn reset(t:&Template,p:&mut Particle,ds:&mut R250,respawn:bool) {
        p.position=position(t,ds);p.velocity=Vec3::new(sample(t,19,20,ds),sample(t,21,22,ds),-sample(t,23,24,ds));p.acceleration=Vec3::Y*f(t,18);p.angle=r(ds)*360.0;p.spin=(r(ds)*2.0-1.0)*f(t,16);p.life=sample(t,42,43,ds);
        if respawn {p.phase=0.0;p.angle=r(ds)*360.0;}
    }
    pub(super) fn configure(&mut self,c:EffectConfig)->Result<()> {if let Some(d)=c.duration {self.duration=d;}Ok(())}
    // The inherited GC100a719a stop byte is not consulted by GC1010a3b5.
    pub(super) fn terminate_gracefully(&mut self) {}
    pub(super) fn update_source(&mut self,source:Mat4)->Result<()> {self.source=sprites::connector(&self.template,source)?;Ok(())}
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {let n=self.particles.len();vec![(Some(w(&self.template,9) as usize),(0..n as u32).flat_map(|i|[i*4,i*4+2,i*4+1,i*4+2,i*4+3,i*4+1]).collect(),n*4)]}
    pub(super) fn blends(&self)->Vec<Blend> {vec![if w(&self.template,0)&0x200!=0 {Blend::Additive}else{Blend::AlphaBlend}]}
    #[allow(clippy::too_many_arguments)]
    pub(super) fn vertices(&mut self,time:f32,camera:Vec3,right:Vec3,up:Vec3,_gc:&mut R250,ds:&mut R250,_crt:&mut CrtRand,terrain:Option<&mut dyn FnMut(Vec3)->Option<(Vec3,Vec3)>>)->Result<Option<Vec<Vec<Vertex>>>> {
        if self.duration>=0.0 && time>self.duration {return Ok(None);}
        self.ground(terrain)?;
        let dt=(time-self.previous).max(0.0);self.previous=time;self.emission+=dt;
        let t=&self.template;let mode=w(t,10);let m=materials::MATERIALS[w(t,9) as usize];
        // DS animation rate is shared: ping-pong reverses it inside the particle loop.
        for p in &mut self.particles {
            p.angle+=dt*p.spin;if p.angle>360.0 {p.angle-=360.0;}else if p.angle<0.0 {p.angle+=360.0;}
            if w(t,39)!=0 {
                if p.frame!= -999.0 {p.frame+=dt*self.frame_rate;}
                if p.frame>m.4 as f32 {match w(t,39) {1=>p.frame=m.3 as f32,2=>p.frame= -999.0,3=>{self.frame_rate= -self.frame_rate;p.frame+=dt*self.frame_rate;},_=>{}}}
                if p.frame<m.3 as f32 && w(t,39)==3 {self.frame_rate= -self.frame_rate;p.frame+=dt*self.frame_rate;}
            }
        }
        for p in &mut self.particles {match w(t,13) {0=>{p.position+=p.velocity*dt;p.velocity+=p.acceleration*dt;},1=>{let a=p.phase.to_radians();p.position=p.origin+Vec3::new(a.sin(),a.sin()*0.4,-a.cos()*0.7);p.phase+=f(t,16)*dt;if p.phase>360.0 {p.phase-=360.0;}},_=>{}}}
        for p in &mut self.particles {match w(t,15) {
            1=>{p.size+=(r(ds)*2.0-1.0)*f(t,17)*dt;if p.size>1.0 || p.size<0.0 {p.size=r(ds);}},
            2=>{p.size+=f(t,17)*dt;if p.size>2.0 {p.size-=2.0;}},
            3..=5=>{p.size-=f(t,17)*dt;if p.size< -1.0 {p.size=if w(t,15)==3 {1.0}else{0.5+r(ds)*0.5};p.delay=r(ds);p.angle=r(ds)*360.0;if w(t,15)>=4 {p.position=position(t,ds);}}},_=>{}}}
        for p in &mut self.particles {match mode {
            7=>{let a=p.phase.to_radians();p.position=p.origin+Vec3::new(a.sin()*p.uv.x,a.sin()*p.uv.y,-a.cos()*p.uv.z);p.phase+=f(t,16)*30.0*dt;if p.phase>360.0 {p.phase-=360.0;}},
            8=>{if p.size==999.0 || p.delay==999.0 {p.size-=f(t,17)*dt;}else {p.delay-=f(t,16)*dt;if p.delay<0.0 {p.size=0.5+r(ds)*0.5;p.delay=999.0;p.angle=r(ds)*360.0;p.position=position(t,ds);}}},
            9=>{if p.phase>=1.0 {if f(t,37)>0.0 && self.emission>f(t,37) {self.emission=0.0;p.phase=0.0;p.angle=r(ds)*360.0;}}else{p.phase+=f(t,17)*dt;}},
            11=>{if p.phase<=0.0 {if f(t,37)>0.0 && self.emission>f(t,37) {self.emission=0.0;p.phase=1.0;p.frame=m.3 as f32+(m.4-m.3) as f32*r(ds);p.angle=r(ds)*360.0;}}else{p.phase-=f(t,17)*dt;}},_=>{}}}
        if mode==12 {if f(t,37)>0.0 && self.emission>f(t,37) {if let Some(p)=self.particles.iter_mut().find(|p|p.life<=0.0) {self.emission=0.0;Self::reset(t,p,ds,true);}}for p in &mut self.particles {if p.life>0.0 {p.life-=dt;}}}
        let phase=if self.duration<0.0 {-1.0}else{time/self.duration};
        let mut color=rgba(w(t,26));let mut width=f(t,33);let mut height=f(t,34);
        let fade_in=f(t,40).max(0.0);let fade_out=f(t,41).max(0.0);
        let segment=if phase>=0.0 && phase<fade_in {Some((25,26,31,33,32,34,phase/if fade_in==0.0 {1.0}else{fade_in}))}else if phase>=0.0 && phase>fade_out {Some((26,27,33,35,34,36,(phase-fade_out)/if 1.0-fade_out==0.0 {1.0}else{1.0-fade_out}))}else{None};
        if let Some((ca,cb,wa,wb,ha,hb,k))=segment {let a=rgba(w(t,ca));let b=rgba(w(t,cb));for i in 0..4 {color[i]=a[i]+(b[i]-a[i])*k;}let k=if w(t,13)==4 {k.powi(8)}else{k};width=f(t,wa)+(f(t,wb)-f(t,wa))*k;height=f(t,ha)+(f(t,hb)-f(t,ha))*k;}
        let mut vertices=Vec::with_capacity(self.particles.len()*4);
        for p in &mut self.particles {
            let pos=self.source.transform_point3(p.position);let distance=pos.distance(camera);let mut alpha=1.0;
            let mut visible=p.life>0.0 && p.frame>=0.0;
            if f(t,28)!=0.0 || f(t,29)!=0.0 || f(t,30)!=0.0 {visible&=distance>=f(t,28)&&distance<=f(t,30);let denominator=if distance<f(t,29) {f(t,29)-f(t,28)}else{f(t,30)-f(t,29)};alpha=1.0-(distance-f(t,29)).abs()/denominator;}
            let mut width=width;let mut height=height;
            match w(t,15) {1=>alpha=alpha.min(p.size),2=>{alpha=alpha.min(if p.size>1.0 {2.0-p.size}else{p.size});},3=>{visible&=p.size>0.0;alpha*=p.size;},4=>{visible&=p.size>0.0;let pulse=(p.size*std::f32::consts::PI).sin();alpha*=pulse;width*=pulse;height*=pulse;},_=>{}}
            if mode==8 {visible&=p.size>0.0&&p.size!=999.0;let pulse=(p.size*std::f32::consts::FRAC_PI_2).sin();alpha*=pulse;width*=pulse;height*=pulse;}
            if mode==9 {visible&=p.phase<1.0&&p.size<=0.0;alpha*=(p.phase*std::f32::consts::PI).sin();}
            if [4,9,10].contains(&mode) {width*=p.phase;height*=p.phase;}
            let angle= -p.angle.to_radians();let x=(right*angle.cos()+up*angle.sin())*width;let y=(-right*angle.sin()+up*angle.cos())*height;
            let positions=match w(t,14) {0=>[pos+x+y,pos-x+y,pos+x-y,pos-x-y],1=>[pos+x+y,pos+y,pos+x,pos],2=>[pos+x*2.0+y*2.0,pos-x*2.0+y*2.0,pos+x,pos-x],_=>unreachable!()};
            if !visible {quad(&mut vertices,positions,[[0.0;2];4],[0.0;4]);continue;}
            // GfxVisual +198 is zero in this constructor, independent of animation +20c.
            let frame=p.frame as i32;let (u,v)=((frame%m.1 as i32) as f32/m.1 as f32,(frame/m.1 as i32) as f32/m.2 as f32);
            let mut uv=if w(t,0)&0x100!=0 {[[0.0,1.0],[1.0,1.0],[0.0,0.0],[1.0,0.0]]}else{[[u,v],[u,v+1.0/m.2 as f32],[u+1.0/m.1 as f32,v],[u+1.0/m.1 as f32,v+1.0/m.2 as f32]]};
            if mode==1 {for uv in &mut uv {uv[0]=1.0-(uv[0]+p.delay);uv[1]+=p.uv.x;}}
            let mut c=color;c[3]=(c[3]*alpha*255.0).round()/255.0;if !visible {c[3]=0.0;}for channel in &mut c[..3] {*channel=channel.clamp(0.0,1.0).powf(2.2);}c[3]=c[3].clamp(0.0,1.0);
            let start=vertices.len();quad(&mut vertices,positions,uv,c);if mode==1 {vertices[start+1].color[3]=0.0;}
        }
        Ok(Some(vec![vertices]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authored_bparticle_modes()->Result<()> {
        let templates=super::super::Templates::open(&ao_gui::client_dir())?;
        let mut gc=R250::new(0xe6f1);let mut ds=R250::new(0xe6f1);let mut crt=CrtRand::new(1);
        let mut count=0;
        for t in templates.by_id.values().filter(|t|t.kind==3024) {
            let mut terrain=|p:Vec3|Some((Vec3::new(p.x,0.0,p.z),Vec3::Y));
            let mut effect=ParticleEffect::new(t,Mat4::IDENTITY,Mat4::IDENTITY,0,&mut gc,&mut ds,&mut crt)?;
            for step in 1..=40 {
                if let Some(groups)=effect.vertices(step as f32*0.01,Vec3::Z*10.0,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt,Some(&mut terrain))? {
                    ensure!(groups[0].len()==w(t,11) as usize*4,"BParticle vertex capacity");
                    for v in groups.iter().flatten() {ensure!(v.pos.iter().chain(v.color.iter()).all(|x|x.is_finite()),"nonfinite authored BParticle");}
                }
            }
            count+=1;
        }
        ensure!(count>0,"no authored BParticle records");
        Ok(())
    }
    #[test]
    fn authored_72340_retains_native_zero_initialized_particles()->Result<()> {
        let templates=super::super::Templates::open(&ao_gui::client_dir())?;
        let t=templates.by_id.get(&72340).context("missing authored BParticle72340")?;
        assert_eq!(w(t,10),13);
        let mut gc=R250::new(0xe6f1);let mut ds=R250::new(0xe6f1);let mut crt=CrtRand::new(1);
        let mut untouched_ds=R250::new(0xe6f1);
        let mut effect=ParticleEffect::new(t,Mat4::IDENTITY,Mat4::IDENTITY,0,&mut gc,&mut ds,&mut crt)?;
        assert_eq!(r(&mut ds),r(&mut untouched_ds),"native default initialization consumes no RNG");
        assert!(effect.particles.iter().all(|p|p.position==Vec3::ZERO && p.life==0.0 && p.spin==0.0));
        let mut terrain=|p:Vec3|Some((Vec3::new(p.x,0.0,p.z),Vec3::Y));
        let groups=effect.vertices(0.01,Vec3::Z*10.0,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt,Some(&mut terrain))?.context("authored72340 expired")?;
        assert_eq!(groups[0].len(),w(t,11) as usize*4);
        assert!(groups[0].iter().all(|v|v.color==[0.0;4]),"DS1000a70f skips life<=0");
        Ok(())
    }
    #[test]
    fn authored_bparticle_native_branch_matrix()->Result<()> {
        let templates=super::super::Templates::open(&ao_gui::client_dir())?;
        let authored=templates.by_id.get(&71343).context("missing authored BParticle71343")?;
        for mode in 0..=13 {for motion in 0..=1 {for geometry in 0..=2 {for size in 0..=5 {
            let mut t=authored.clone();t.words[10]=mode;t.words[13]=motion;t.words[14]=geometry;t.words[15]=size;
            let mut gc=R250::new(0xe6f1);let mut ds=R250::new(0xe6f1);let mut crt=CrtRand::new(1);
            let mut terrain=|p:Vec3|Some((Vec3::new(p.x,0.0,p.z),Vec3::Y));
            let mut effect=ParticleEffect::new(&t,Mat4::IDENTITY,Mat4::IDENTITY,0,&mut gc,&mut ds,&mut crt)?;
            for step in 1..=40 {
                if let Some(groups)=effect.vertices(step as f32*0.01,Vec3::Z*10.0,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt,Some(&mut terrain))? {
                    for v in groups.iter().flatten() {ensure!(v.pos.iter().chain(v.color.iter()).all(|x|x.is_finite()),"nonfinite BParticle mode{mode}/motion{motion}/geometry{geometry}/size{size}");}
                }
            }
        }}}}
        Ok(())
    }
    #[test]
    #[ignore = "requires installed retail data and offscreen GPU"]
    fn authored_bparticle_offscreen()->Result<()> {
        use super::super::{Binding,Creation,Renderer,MODEL_BASE};
        let out=std::env::var_os("AOMAC_EFFECT_FRAMES").map(std::path::PathBuf::from).context("set AOMAC_EFFECT_FRAMES")?;
        std::fs::create_dir_all(&out)?;
        let templates=super::super::Templates::open(&ao_gui::client_dir())?;
        let mut renderer=Renderer::open(&ao_gui::client_dir())?;let mut host=ao_render::Host::headless();
        let mut visible_pixels=0;
        for (&id,t) in &templates.by_id {
            if t.kind!=3024 {continue;}
            renderer.clear();host.camera=ao_render::Camera::look_at(Vec3::new(2.0,2.0,5.0),Vec3::ZERO);
            renderer.spawn(Binding {group:0,attractor:0,effect:id,note:0,color:0},Creation::Vector,Mat4::IDENTITY,Vec3::X)?;
            for step in 1..=40 {
                let mut terrain=|p:Vec3|Some((Vec3::new(p.x,0.0,p.z),Vec3::Y));
                host.actors.clear();renderer.frame(0.01,&mut host,Some(&mut terrain));
                if ![1,10,20,40].contains(&step) {continue;}
                let models:Vec<_>=renderer.models.iter().map(|(&id,m)|(MODEL_BASE|id as u32 as u64,m.scene.clone())).collect();
                let path=out.join(format!("bparticle3024_{id}_{step}.png"));
                let blank=out.join(format!("bparticle3024_blank_{id}_{step}.png"));
                let time=step as f32*0.01;
                ao_render::render_to_png_actors(&ao_scene::Scene::default(),&models,host.actors.clone(),[2.0,2.0,5.0],[0.0;3],640,480,&path,time)?;
                ao_render::render_to_png_actors(&ao_scene::Scene::default(),&[],Vec::new(),[2.0,2.0,5.0],[0.0;3],640,480,&blank,time)?;
                let image=image::open(&path)?.to_rgba8();let background=image::open(&blank)?.to_rgba8();
                let pixels=image.pixels().zip(background.pixels()).filter(|(a,b)|a!=b).count();
                visible_pixels+=pixels;
                eprintln!("BParticle3024 {id} frame{step}: {pixels} resource pixels");
                std::fs::remove_file(blank)?;
            }
        }
        ensure!(visible_pixels>0,"authored BParticle records produced no GPU resource pixels");
        Ok(())
    }
}
