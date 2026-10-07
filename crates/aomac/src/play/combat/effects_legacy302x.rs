//! GC SkyRise3018:100efd12/100efe2c, Trail3019:101020db..1010252d,
//! BufferControl3021:1010c32c, EnergyBall3023:1010dfa5..1010e107,
//! MParticle3027:1010fea9..10110119. Parameters are installed gfxtweak words.
use super::{EffectConfig, Template};
use anyhow::{ensure, Context, Result};
use ao_formats::{character::{CrtRand, NameTable}, mesh::{decode_mesh_into, MESH_TYPE}, weather::R250};
use ao_rdb::RecordStore;
use ao_scene::{Blend, Scene, Vertex};
use glam::{Mat4, Quat, Vec3};
use std::sync::Arc;

fn fraction(r:&mut R250)->f32 {super::random_fraction(r)}
fn range(r:&mut R250,a:f32,b:f32)->f32 {a+(b-a)*fraction(r)}
fn rgba(w:u32)->[f32;4] {let [a,r,g,b]=w.to_be_bytes();[r as f32/255.0,g as f32/255.0,b as f32/255.0,a as f32/255.0]}
fn color(a:u32,b:u32,u:f32)->[f32;4] {let a=a.to_be_bytes();let b=b.to_be_bytes();rgba(u32::from_be_bytes(std::array::from_fn(|i|(a[i] as f32*(1.0-u)+b[i] as f32*u) as u8)))}
fn vertex(p:Vec3,uv:[f32;2],mut c:[f32;4])->Vertex {for v in &mut c[..3] {*v=v.clamp(0.0,1.0).powf(2.2);}Vertex {pos:p.to_array(),normal:[0.0,1.0,0.0],uv,color:c}}
fn strip(n:u32)->Vec<u32> {(0..n.saturating_sub(2)).flat_map(|i|if i&1==0 {[i,i+1,i+2]}else{[i+1,i,i+2]}).collect()}

struct MeshParticle {resource:usize,position:Vec3,velocity:Vec3,axis:Vec3,angle:f32,spin:f32,scale:f32,remaining:f32,life:f32,alpha:f32}
pub(super) struct MParticle {t:Template,source:Mat4,elapsed:f32,emission:f32,started:bool,particles:Vec<MeshParticle>,resources:Vec<(u32,Arc<Scene>)>}
impl MParticle {
    pub(super) fn resource_names(t:&Template)->Result<Vec<&'static str>> {
        let n=t.word(33)? as usize;ensure!(n>0&&n<=10,"invalid MParticle resource count");
        // GC1010ef6f: mesh identities, not rock/environment selectors.
        const NAMES:[&str;10]=["EP03_shoulder_rocket.abiff","EP03_mech_heal_effect.abiff","EP03_blast_wave_effect.abiff","EP03_bullet_casing.abiff","EP03_EMP_blast.abiff","EP03_bomber_debris_01.abiff","EP03_bomber_debris_02.abiff","EP03_bomber_debris_03.abiff","EP03_RUBIKA_asteroidbig.abiff","EP03_RUBIKA_asteroidbig2.abiff"];
        (0..n).map(|i|NAMES.get(t.word(34+i)? as usize).copied().context("invalid MParticle mesh selector")).collect()
    }
    pub(super) fn new(t:&Template,source:Mat4,store:&RecordStore,names:&NameTable,crt:&mut CrtRand,r:&mut R250)->Result<Self> {
        ensure!(t.kind==3027,"not MParticle");ensure!(t.word(9)?<=1,"invalid MParticle mode");
        for i in 12..=32 {t.float(i)?;}
        let resource_names=Self::resource_names(t)?;let n=resource_names.len();
        let count=t.word(11)? as usize;ensure!(count<=65535,"invalid MParticle capacity");
        let mut resources=Vec::with_capacity(n);
        for name in resource_names {
            let id=names.id(MESH_TYPE,name).with_context(||format!("missing MParticle mesh {name}"))?;
            // GC1010fcd8 allocates every VisualMesh before asynchronous SetMesh.
            let mut scene=Scene::default();
            if let Some(mesh)=decode_mesh_into(store,id,&mut scene)? {
                scene.instances.push(ao_scene::Instance {mesh,transform:Mat4::IDENTITY.to_cols_array_2d()});
            }
            resources.push((id,Arc::new(scene)));
        }
        let mut t=t.clone();
        // GC1010f09d chooses the zero-axis replacement at construction.
        if t.float(23)?==0.0&&t.float(24)?==0.0&&t.float(25)?==0.0 {for i in 23..=25 {t.words[i]=(fraction(r)*2.0-1.0).to_bits();}}
        let particles=(0..count).map(|_|MeshParticle {resource:crt.rand() as usize%n,position:Vec3::ZERO,velocity:Vec3::ZERO,axis:Vec3::Y,angle:0.0,spin:0.0,scale:0.0,remaining:0.0,life:0.0,alpha:0.0}).collect();
        Ok(Self {t,source,elapsed:0.0,emission:0.0,started:false,particles,resources})
    }
    pub(super) fn configure(&mut self,c:EffectConfig) {if let Some(d)=c.duration {self.t.words[8]=d.to_bits();}}
    pub(super) fn update_source(&mut self,m:Mat4) {self.source=m;}
    pub(super) fn resources(&self)->&[(u32,Arc<Scene>)] {&self.resources}
    pub(super) fn actor_count(&self)->usize {self.particles.len()}
    pub(super) fn instances(&self)->impl Iterator<Item=(usize,Mat4,f32)>+'_ {self.particles.iter().map(|p|(p.resource,Mat4::from_scale_rotation_translation(Vec3::splat(p.scale),Quat::from_axis_angle(p.axis,p.angle),p.position),if p.remaining>0.0 {p.alpha}else{0.0}))}
    pub(super) fn actors(&self,actor_base:u32,model_base:u64)->Vec<ao_scene::ActorFrame> {
        self.instances().enumerate().filter(|(_, (resource,_,alpha))|*alpha>0.0 && !self.resources[*resource].1.meshes.is_empty()).map(|(index,(resource,transform,alpha))|ao_scene::ActorFrame {
            id:actor_base+index as u32,model:model_base|u64::from(self.resources[resource].0),transform:transform.to_cols_array_2d(),alpha,..Default::default()
        }).collect()
    }
    fn reset(t:&Template,p:&mut MeshParticle,source:Mat4,r:&mut R250,oriented:bool)->Result<()> {
        p.scale=range(r,t.float(30)?,t.float(31)?);p.life=range(r,t.float(28)?,t.float(29)?);p.remaining=p.life;
        let radius=t.float(13)?;
        p.position=source.w_axis.truncate()+Vec3::new(range(r,-radius,radius),range(r,-radius,radius),-range(r,-radius,radius));
        p.velocity=Vec3::new(range(r,t.float(17)?,t.float(18)?),range(r,t.float(19)?,t.float(20)?),-range(r,t.float(21)?,t.float(22)?));
        let axis=Vec3::new(-t.float(23)?,-t.float(24)?,t.float(25)?);
        // GC1003deb7 detects the authored zero axis, selecting random XYZ.
        p.axis=axis.try_normalize().unwrap_or(Vec3::Y);p.angle=fraction(r)*std::f32::consts::TAU;
        p.spin=range(r,t.float(26)?.to_radians(),t.float(27)?.to_radians());
        if oriented {p.velocity=source.transform_vector3(p.velocity);p.axis=source.transform_vector3(p.axis).normalize_or_zero();}
        p.alpha=0.0;Ok(())
    }
    pub(super) fn frame(&mut self,dt:f32,r:&mut R250,ground:&mut dyn FnMut(Vec3)->Option<(Vec3,Vec3)>)->Result<bool> {
        ensure!(dt.is_finite()&&dt>=0.0,"invalid MParticle timestep");self.elapsed+=dt;self.emission+=dt;
        let mut source=self.source;
        if self.t.words[0]&0x100!=0 {source.w_axis.y=ground(source.w_axis.truncate()).map_or(0.0,|(p,_)|p.y);}
        if !self.started {
            for p in &mut self.particles {Self::reset(&self.t,p,source,r,false)?;}
            if self.t.words[9]==1 {for (i,p) in self.particles.iter_mut().enumerate(){p.remaining=if i==0 {self.t.float(29)?}else{0.0};}}
            self.started=true;
        }
        let mut alive=false;
        for p in &mut self.particles {
            p.remaining-=dt;
            if p.remaining<=0.0 {p.remaining=0.0;p.alpha=0.0;continue;}
            alive=true;
            if self.t.words[0]&0x800!=0 && p.position.y<=ground(p.position).map_or(0.0,|(p,_)|p.y) {
                let friction=self.t.float(32)?;p.velocity.x-=friction*p.velocity.x*dt*100.0;p.velocity.y*=friction*dt*-100.0;p.velocity.z-=friction*p.velocity.z*dt*100.0;
                p.spin=range(r,self.t.float(26)?.to_radians(),self.t.float(27)?.to_radians())*(p.velocity.y*0.1).abs()*friction;
            }
            p.position+=p.velocity*dt;p.velocity.y+=self.t.float(16)?*dt;p.angle+=p.spin*dt;
            let phase=1.0-p.remaining/p.life;
            p.alpha=if phase<self.t.float(14)? {phase/self.t.float(14)?}else if phase>1.0-self.t.float(15)? {1.0-(phase-(1.0-self.t.float(15)?))/self.t.float(15)?}else{1.0};
        }
        if self.t.words[9]==0 {return Ok(alive);}
        let duration=self.t.float(8)?;
        if duration>0.0&&self.elapsed>=duration {return Ok(false);}
        if self.elapsed<duration-self.t.float(29)? && self.t.float(12)?>0.0 && self.emission>self.t.float(12)? {
            if let Some(p)=self.particles.iter_mut().find(|p|p.remaining<=0.0) {self.emission=0.0;Self::reset(&self.t,p,source,r,true)?;}
        }
        Ok(true)
    }
}

/// GC1010c296: lifetime follows the live RConnector. DS1000bc0b clears viewport.
pub(super) struct BufferControl {alive:bool}
impl BufferControl {
    pub(super) fn new(t:&Template,connector:Option<Mat4>)->Result<Self> {ensure!(t.kind==3021,"not BufferControl");Ok(Self {alive:connector.is_some()})}
    pub(super) fn frame(&mut self,connector:Option<Mat4>)->bool {self.alive&=connector.is_some();self.alive}
    pub(super) fn clears_viewport(&self,visible:bool)->bool {self.alive&&visible}
}

pub(super) struct EnergyBall {t:Template,source:Mat4,position:Vec3,elapsed:f32,started:bool}
impl EnergyBall {
    pub(super) fn new(t:&Template,source:Mat4,r:&mut R250,ground:&mut dyn FnMut(Vec3)->Option<(Vec3,Vec3)>)->Result<Self> {
        ensure!(t.kind==3023,"not EnergyBall");t.word(26)?;
        for i in [8,10,11,13,14,15,16,17,23,24,25,26] {t.float(i)?;}
        ensure!((1..=1024).contains(&t.word(12)?)&&t.word(22)?<=2,"invalid EnergyBall mode/count");
        let mut position=source.w_axis.truncate();
        if t.float(26)?>0.0 {position+=Vec3::new(fraction(r)*2.0-1.0,fraction(r)*2.0-1.0,-(fraction(r)*2.0-1.0))*t.float(26)?;}
        if t.words[0]&0x2000!=0 {position.y=ground(position).map_or(0.0,|(p,_)|p.y);}
        Ok(Self {t:t.clone(),source,position,elapsed:0.0,started:false})
    }
    pub(super) fn update_source(&mut self,m:Mat4) {self.source=m;}
    pub(super) fn configure(&mut self,c:EffectConfig) {if let Some(d)=c.duration {self.t.words[8]=d.to_bits();}}
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {vec![(Some(self.t.words[9] as usize),(0..self.t.words[12]*3).flat_map(|i|[i*4,i*4+1,i*4+2,i*4+2,i*4+1,i*4+3]).collect(),self.t.words[12] as usize*12)]}
    pub(super) fn blends(&self)->Vec<Blend> {vec![Blend::Additive]}
    pub(super) fn frame(&mut self,dt:f32)->Result<bool> {ensure!(dt.is_finite()&&dt>=0.0,"invalid EnergyBall timestep");if self.started {self.elapsed+=dt;}else{self.started=true;}Ok(self.elapsed<self.t.float(10)?+self.t.float(11)? && (self.t.float(8)?<=0.0||self.elapsed<self.t.float(8)?))}
    pub(super) fn vertices(&self)->Result<Vec<Vec<Vertex>>> {
        let t=&self.t;let grow=t.float(10)?;let shrink=t.float(11)?;let phase=self.elapsed/(grow+shrink);
        let rising=self.elapsed<grow;
        let raw=if rising {self.elapsed/grow}else{1.0-(self.elapsed-grow)/shrink};
        let u=match t.words[22] {0=>1.0-(1.0-raw).powi(6),1=>raw.powi(6),_=>raw};
        let mut radius=if rising {t.float(15)?*(1.0-u)+t.float(16)?*u}else{t.float(17)?*(1.0-u)+t.float(16)?*u};
        radius+=(phase*std::f32::consts::TAU).cos()*t.float(24)?;
        let h=t.float(25)?;
        let (height,scale)=if h>0.0 {(h-h*phase.powi(8),phase)}else if h<0.0 {(h*(1.0-phase).powi(4)-h,1.0-phase)}else{(0.0,1.0)};
        radius*=scale;
        let p=self.position+Vec3::new((phase*8.0*std::f32::consts::PI).sin(),(phase*6.0*std::f32::consts::PI).cos(),-(phase*std::f32::consts::TAU).sin())*t.float(23)?+Vec3::Y*height;
        let axis=Vec3::new(-(phase*std::f32::consts::FRAC_PI_2).cos(),-(phase*1.5*std::f32::consts::PI).cos(),(phase*3.0*std::f32::consts::PI).sin()).normalize_or_zero();
        let q=Quat::from_axis_angle(axis,phase*std::f32::consts::FRAC_PI_2);
        let rotation=Mat4::from_quat(q)*Mat4::from_cols(self.source.x_axis,self.source.y_axis,self.source.z_axis,glam::Vec4::W);
        let c0=color(t.words[18],t.words[20],u);let c1=color(t.words[19],t.words[21],u);
        let mut out=Vec::with_capacity(t.words[12] as usize*12);
        // DS10012264: three independently rotated planes per angular step.
        for i in 0..t.words[12] {let a=std::f32::consts::PI*i as f32/t.words[12] as f32;
            for plane in 0..3 {let rq=match plane {0=>Quat::from_rotation_x(a),1=>Quat::from_rotation_y(a),_=>Quat::from_rotation_z(a)};
                let right=rq*if plane==2 {Vec3::Z}else{Vec3::X};let up=rq*Vec3::Y;
                for (j,(x,y)) in [(1.0,1.0),(-1.0,1.0),(1.0,-1.0),(-1.0,-1.0)].into_iter().enumerate() {
                    let uv=if t.words[0]&0x100!=0 {[j as f32%2.0,if j<2 {1.0}else{0.0}]}else{[j as f32%2.0,if j<2 {0.0}else{1.0}]};
                    out.push(vertex(p+rotation.transform_vector3((right*x+up*y)*radius),uv,if j<2 {c0}else{c1}));
                }
            }
        }
        Ok(vec![out])
    }
}

pub(super) struct SkyRise {t:Template,position:Vec3,elapsed:f32,started:bool}
impl SkyRise {
    pub(super) fn new(t:&Template,source:Mat4,ground:&mut dyn FnMut(Vec3)->Option<(Vec3,Vec3)>)->Result<Self> {
        ensure!(t.kind==3018,"not SkyRise");t.word(25)?;
        for i in [8,11,12,15,17,18,19] {t.float(i)?;}
        ensure!((6..=512).contains(&t.word(14)?)&&t.word(24)?<=1,"invalid SkyRise geometry");
        let mut position=source.w_axis.truncate();if t.words[0]&0x4000!=0 {position.y=ground(position).map_or(0.0,|(p,_)|p.y);}
        Ok(Self {t:t.clone(),position,elapsed:0.0,started:false})
    }
    pub(super) fn update_source(&mut self,m:Mat4) {if self.t.words[0]&0x1000!=0 {self.position=m.w_axis.truncate();}}
    pub(super) fn configure(&mut self,c:EffectConfig) {if let Some(d)=c.duration {self.t.words[8]=d.to_bits();}}
    fn count(&self)->u32 {if self.t.words[24]==1 {self.t.words[14]*2}else{14}}
    fn bands(&self)->u32 {if self.t.words[24]==1 {self.t.words[14]/2}else{2}}
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {
        let n=self.count();let bands=self.bands();
        (0..2).map(|_|(Some(self.t.words[9] as usize),(0..bands).flat_map(|b|strip(n).into_iter().map(move|i|i+b*n)).collect(),(n*bands) as usize)).collect()
    }
    pub(super) fn blends(&self)->Vec<Blend> {vec![Blend::Additive;2]}
    pub(super) fn frame(&mut self,dt:f32)->Result<bool> {ensure!(dt.is_finite()&&dt>=0.0,"invalid SkyRise timestep");if self.started {self.elapsed+=dt;}else{self.started=true;}Ok(self.elapsed<self.t.float(8)?)}
    pub(super) fn vertices(&self)->Result<Vec<Vec<Vertex>>> {
        let t=&self.t;let phase=self.elapsed/t.float(8)?;
        let opacity=if self.elapsed<t.float(11)? {(1.0-self.elapsed/t.float(11)?)*0.25+0.75}else{(1.0-(self.elapsed-t.float(11)?)/(t.float(8)?-t.float(11)?))*0.75};
        let alpha=[(1.0-(1.0-opacity)*4.0).max(0.0),(opacity*1.33).min(1.0),(1.0-(1.0-opacity)*4.0).max(0.0)];
        let radius=if t.words[25]&1!=0 {t.float(18)?*(1.0-phase)+t.float(19)?*phase}else{t.float(18)?};
        let mut c=[rgba(t.words[20]),rgba(t.words[21])];
        if t.words[24]==1&&t.words[25]&2!=0 {c=[color(t.words[20],t.words[21],phase),color(t.words[22],t.words[23],phase)];}
        if t.words[24]==1&&t.words[25]&4!=0 {
            let cycles=(t.float(8)?/t.float(12)?).floor();let triangle=|u:f32|{let x=u.fract()*2.0;if x>1.0 {2.0-x}else{x}};
            c=[color(t.words[20],t.words[21],triangle(cycles*phase)),color(t.words[22],t.words[23],triangle((cycles+1.0)*phase))];
        }
        let mut out=Vec::with_capacity(2);
        let u=0.2*self.elapsed+0.2*self.elapsed.sin();let v=0.6*self.elapsed;
        for (pass,pass_color) in c.into_iter().enumerate() {
            let mut vertices=Vec::with_capacity((self.count()*self.bands()) as usize);
            let (scroll_u,scroll_v,vertical,texture_scale)=if pass==0 {(u,v,t.words[0]&0x100!=0,4.0)}else{(v*-0.9,u*-1.3,t.words[0]&0x100==0,5.0)};
            if t.words[24]==1 {
                let n=t.words[14];let bands=n/2;
                for band in 0..bands {for i in 0..n {let azimuth=i as f32*std::f32::consts::TAU/(n-1) as f32;
                    for end in 0..2 {let elevation=(band+end) as f32*f32::from_bits(0x3fc90cb3)/bands as f32;let p=self.position+Vec3::new(azimuth.sin()*elevation.cos(),elevation.sin(),-azimuth.cos()*elevation.cos())*radius;
                        let x=i as f32*(radius*f32::from_bits(0x40c8f5c3)/t.float(15)?).floor()/n as f32;
                        let y=elevation*radius/t.float(15)?/texture_scale;
                        let uv=if vertical {[y+scroll_u,x+scroll_v]}else{[x+scroll_u,y+scroll_v]};
                        vertices.push(vertex(p,uv,pass_color));
                    }
                }}
            } else {
                let xs:[f32;6]=[5.0,5.0,-5.0,-5.0,0.0,0.0];let zs=[5.0,-5.0,-5.0,0.0,0.0,5.0];let height=t.float(17)?;let levels=[0.0,(height*0.5).min(1.0),height];
                for band in 0..2 {let mut distance=0.0;for i in 0..=6 {let j=(i+5)%6;if i>0 {let prev=(j+5)%6;distance+=(xs[j]-xs[prev]).hypot(zs[j]-zs[prev]);}
                    for end in 0..2 {let y=levels[band+end];let mut color=color(t.words[21],t.words[20],(band+end) as f32*0.5);color[3]=alpha[band+end];
                        let uv=if vertical {[y/texture_scale+scroll_u,distance/texture_scale+scroll_v]}else{[distance/texture_scale+scroll_u,y/texture_scale+scroll_v]};
                        vertices.push(vertex(self.position+Vec3::new(xs[j],y,-zs[j]),uv,color));
                    }
                }}
            }
            out.push(vertices);
        }
        Ok(out)
    }
}

struct TrailSample {source:Mat4,time:f32,distance:f32}
pub(super) struct Trail {t:Template,source:Mat4,elapsed:f32,last_sample:f32,samples:std::collections::VecDeque<TrailSample>,distance:f32,stop:bool}
impl Trail {
    pub(super) fn new(t:&Template,source:Mat4)->Result<Self> {
        ensure!(t.kind==3019,"not Trail");for i in [8,10,11,12] {t.float(i)?;}
        ensure!(t.words.get(14).copied().unwrap_or(0)<=1&&t.float(11)?>0.0&&t.float(12)?>0.0,"invalid Trail mode/timing");
        Ok(Self {t:t.clone(),source,elapsed:0.0,last_sample:0.0,samples:std::collections::VecDeque::with_capacity(1000),distance:0.0,stop:false})
    }
    pub(super) fn update_source(&mut self,m:Mat4) {self.source=m;}
    pub(super) fn configure(&mut self,c:EffectConfig) {if let Some([r,g,b,a])=c.start_color {self.t.words[13]=u32::from_be_bytes([ (a.clamp(0.0,1.0)*255.0) as u8,(r.clamp(0.0,1.0)*255.0) as u8,(g.clamp(0.0,1.0)*255.0) as u8,(b.clamp(0.0,1.0)*255.0) as u8]);}}
    pub(super) fn terminate_gracefully(&mut self) {self.stop=true;}
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {(0..if self.t.words.get(14).copied().unwrap_or(0)==1 {4}else{1}).map(|_|(Some(self.t.words[9] as usize),strip(104),104)).collect()}
    pub(super) fn blends(&self)->Vec<Blend> {vec![Blend::Additive;self.models().len()]}
    pub(super) fn frame(&mut self,dt:f32)->Result<bool> {
        ensure!(dt.is_finite()&&dt>=0.0,"invalid Trail timestep");self.elapsed+=dt;
        // DS1002b486 samples once after >50ms, not a catch-up emission loop.
        if self.last_sample+0.05<self.elapsed {let p=self.source.w_axis.truncate();let d=self.samples.back().map_or(0.0,|s|p.distance(s.source.w_axis.truncate()));self.distance+=d;if self.samples.len()==1000 {self.samples.pop_front();}self.samples.push_back(TrailSample {source:self.source,time:self.elapsed,distance:self.distance});self.last_sample=self.elapsed;}
        // GC10101ddb has no lifetime check; graceful only writes +40.
        Ok(true)
    }
    pub(super) fn vertices(&self,camera:Vec3)->Result<Vec<Vec<Vertex>>> {
        let count=if self.t.words.get(14).copied().unwrap_or(0)==1 {4}else{1};let mut out=Vec::with_capacity(count);let width=self.t.float(10)?;let newest=self.samples.back();
        for plane in 0..count {let mut v=Vec::with_capacity(104);let Some(newest)=newest else{out.push(v);continue;};let p=newest.source.w_axis.truncate();
            let mut travel=0.0;let mut index=self.samples.len()-1;let mut previous=p;
            for point in 0..=50 {let distance=newest.distance-point as f32*2.5;
                while index>0&&self.samples[index-1].distance>distance {index-=1;}
                let current=&self.samples[index];let older=&self.samples[index.saturating_sub(1)];let u=if current.distance>older.distance {(distance-older.distance)/(current.distance-older.distance)}else{0.0};
                let position=older.source.w_axis.truncate().lerp(current.source.w_axis.truncate(),u.clamp(0.0,1.0));let time=older.time+(current.time-older.time)*u.clamp(0.0,1.0);
                let alpha=(1.0-(self.elapsed-time)/self.t.float(12)?).max(0.0);
                let x=older.source.x_axis.truncate().lerp(current.source.x_axis.truncate(),u);let z=older.source.z_axis.truncate().lerp(current.source.z_axis.truncate(),u);
                let offset=if count==1 {(previous-position).cross(camera-position).try_normalize().unwrap_or(Vec3::X)*width*0.5*0.97f32.powi(point)}else{match plane {0=>z*width*0.5,1=>x*width*0.5,2=>(x+z).normalize_or_zero()*width*0.35,_=>(x-z).normalize_or_zero()*width*0.35}};
                travel+=previous.distance(position);let mut c=rgba(self.t.words[13]);c[3]*=alpha;
                for side in 0..2 {v.push(vertex(position+offset*if side==0 {1.0}else{-1.0},[(newest.distance-travel)/self.t.float(11)?,side as f32],c));}
                previous=position;if alpha==0.0||index<2 {break;}
            }
            out.push(v);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sky(mode:u32,flags:u32)->Template {Template {kind:3018,words:vec![0x203,0,0,0,0,0,0,0,0x3f19999a,47,47,0x40400000,0x41200000,1,10,0x40900000,0,0x447a0000,0x3dcccccd,0x41700000,0x80c07ed6,0x7ed6,0x80c07ed6,0xc27e,mode,flags]}}
    fn ball(mode:u32)->Template {Template {kind:3023,words:vec![0xf03,0,0,0,0,0,0,0,0xbf800000,8,0x3dcccccd,0x3dcccccd,4,0x3f800000,0x42480000,0,0x3f000000,0,0xffffff,0xffffff,0xffffffff,0xffffffff,mode,0,0,0,0x3e800000]}}
    #[test]
    fn authored_legacy302x_geometry_and_lifecycle() {
        let mut terrain=|p:Vec3|Some((Vec3::new(p.x,0.0,p.z),Vec3::Y));
        for (mode,flags) in [(0,0),(1,5),(1,3)] {
            let mut s=SkyRise::new(&sky(mode,flags),Mat4::IDENTITY,&mut terrain).unwrap();
            assert!(s.frame(0.01).unwrap());assert!(s.frame(0.2).unwrap());
            let v=s.vertices().unwrap();assert_eq!(v.len(),2);assert_eq!(v[0].len(),s.models()[0].2);
            assert!(v.iter().flatten().all(|v|v.pos.iter().chain(v.uv.iter()).all(|v|v.is_finite())));
            assert!(!s.frame(0.5).unwrap());
        }
        let mut r=R250::new(0xe6f1);
        for mode in [0,2] {let mut b=EnergyBall::new(&ball(mode),Mat4::IDENTITY,&mut r,&mut terrain).unwrap();assert!(b.frame(0.01).unwrap());assert!(b.frame(0.05).unwrap());assert_eq!(b.vertices().unwrap()[0].len(),48);assert!(!b.frame(0.2).unwrap());}
        let empty=Template {kind:3021,words:vec![]};let mut buffer=BufferControl::new(&empty,Some(Mat4::IDENTITY)).unwrap();
        assert!(buffer.clears_viewport(true));assert!(!buffer.clears_viewport(false));assert!(!buffer.frame(None));assert!(!buffer.frame(Some(Mat4::IDENTITY)));
    }
    #[test]
    fn authored_12570_trail_missing_mode_defaults_to_zero() {
        let t=Template {kind:3019,words:vec![7,0,0,0,0,0,0,2001,0xbf800000,54,0x40000000,0x40600000,0x40400000,u32::MAX]};
        let mut trail=Trail::new(&t,Mat4::IDENTITY).unwrap();assert_eq!(trail.models().len(),1);
        for i in 0..20 {trail.update_source(Mat4::from_translation(Vec3::X*i as f32));assert!(trail.frame(0.06).unwrap());}
        assert!(!trail.vertices(Vec3::Z*10.0).unwrap()[0].is_empty());trail.terminate_gracefully();assert!(trail.frame(30.0).unwrap());
    }
    #[test]
    fn authored_71250_mesh_particle_reset_preserves_random_order() {
        let t=Template {kind:3027,words:vec![0x803,0,0xbe99999a,0,0,0,0,2000,0x40400000,1,0,40,0x3d23d70a,0,0,0x3e4ccccd,0xc2700000,0x40000000,0x40a00000,0x41700000,0x41800000,0,0x40000000,0,0,0,0,0x43960000,0x3f800000,0x3f800000,0x3f800000,0x3f800000,0x3f000000,1,3]};
        let mut p=MeshParticle {resource:0,position:Vec3::ZERO,velocity:Vec3::ZERO,axis:Vec3::Y,angle:0.0,spin:0.0,scale:0.0,remaining:0.0,life:0.0,alpha:0.0};
        let mut r=R250::new(0xe6f1);MParticle::reset(&t,&mut p,Mat4::IDENTITY,&mut r,true).unwrap();
        assert_eq!(p.scale,1.0);assert_eq!(p.life,1.0);assert!((2.0..=5.0).contains(&p.velocity.x));assert!((15.0..=16.0).contains(&p.velocity.y));assert!((-2.0..=0.0).contains(&p.velocity.z));
    }
    #[test]
    #[ignore = "requires installed retail assets"]
    fn installed_mparticle_missing_heal_keeps_slots_and_lifetime()->Result<()> {
        let dir=ao_gui::client_dir();let store=RecordStore::open(&dir)?;
        let names=NameTable::load(&store)?;let templates=super::super::Templates::open(&dir)?;
        // Exercise native selector1 on actual71250 against71123's real asset hole.
        let mut t=templates.by_id[&71250].clone();t.words[34]=1;
        let mut crt=CrtRand::new(1);let mut rng=R250::new(0xe6f1);
        let mut effect=MParticle::new(&t,Mat4::IDENTITY,&store,&names,&mut crt,&mut rng)?;
        assert_eq!(effect.resources.len(),1);assert!(effect.resources[0].1.meshes.is_empty());
        assert_eq!(effect.actor_count(),t.word(11)? as usize);
        let mut ground=|_:Vec3|None;
        assert!(effect.frame(1.0/60.0,&mut rng,&mut ground)?);
        assert!(effect.particles[0].remaining>0.0);
        assert!(effect.actors(1,0).is_empty(),"retained slots have no drawable payload");
        t.words[28]=f32::NAN.to_bits();
        assert!(MParticle::new(&t,Mat4::IDENTITY,&store,&names,&mut crt,&mut rng).is_err());
        Ok(())
    }
    #[test]
    #[ignore="installed authored legacy302x assets and offscreen GPU"]
    fn retail_legacy302x_authored_frames()->Result<()> {
        use super::super::{Binding,Renderer,MODEL_BASE,MESH_MODEL_BASE};
        let out=std::path::PathBuf::from(std::env::var_os("AOMAC_EFFECT_FRAMES").context("AOMAC_EFFECT_FRAMES required")?);std::fs::create_dir_all(&out)?;
        let mut r=Renderer::open(&ao_gui::client_dir())?;
        let origin=Vec3::new(5000.0,10.0,5000.0);let eye=origin+Vec3::new(50.0,30.0,50.0);
        let mut host=ao_render::Host::headless();host.camera=ao_render::Camera::look_at(eye,origin);
        let mut ids:Vec<_>=r.templates.by_id.iter().filter_map(|(&id,t)|matches!(t.kind,3018|3019|3021|3023|3027).then_some(id)).collect();ids.sort_unstable();
        ensure!(ids.len()==17,"installed legacy302x authored census changed");
        for id in ids {
            r.clear();let t=&r.templates.by_id[&id];let kind=t.kind;
            let source=Mat4::from_translation(origin);
            let config=EffectConfig {creation:if kind==3021 {super::super::Creation::RConnector}else{super::super::Creation::Vector},resource_connector:Some(source),..Default::default()};
            let handle=r.spawn_configured(Binding {group:0,attractor:0,effect:id,note:0,color:0},source,origin,config)?;
            ensure!(handle!=0,"installed legacy302x failed spawning {id}");
            for frame in 1..=180 {
                host.actors.clear();host.viewport_depth_clears.clear();
                if kind==3019 {r.update_source(handle,Mat4::from_translation(origin+Vec3::new(frame as f32*0.5,3.0*(frame as f32/30.0).sin(),0.0)));}
                let mut ground=|p:Vec3|Some((Vec3::new(p.x,10.0,p.z),Vec3::Y));
                r.frame(1.0/60.0,&mut host,Some(&mut ground));
                if kind==3021 {ensure!(!host.viewport_depth_clears.is_empty(),"native BufferControl did not emit depth clear");}
                if [1,6,15,30,60,120,180].contains(&frame)&&kind!=3021 {
                    let mut models:Vec<_>=r.models.iter().map(|(&id,m)|(MODEL_BASE|id as u32 as u64,m.scene.clone())).collect();
                    models.extend(r.buff_models.iter().map(|(&id,m)|(0xfac2_0000_0000_0000|u64::from(id),m.scene.clone())));
                    models.extend(r.mesh_resources.iter().map(|(&id,m)|(MESH_MODEL_BASE|u64::from(id),(**m).clone())));
                    ao_render::render_to_png_actors(&Scene::default(),&models,host.actors.clone(),eye.to_array(),origin.to_array(),640,480,&out.join(format!("legacy302x_{id}_{frame}.png")),frame as f32/60.0)?;
                }
            }
        }
        Ok(())
    }
}
