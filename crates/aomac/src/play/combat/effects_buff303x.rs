//! Native authored Scatter (3029): GC 101107b5/10110864/10110357/10110771.
//! Four scheduled Shield2 children in 73006; this controller draws no geometry.
//! Parameters: Scatter flags/offset[1..3]/spatial mode4/schedule5/count6/
//! cycle7/radius8/ring step9/child10. Persistent73006 uses mode0, four72266.
//! Grid common connector[0..7], lifetime8/material9/modes10..11/scale12/
//! spin13/plane counts14..16, then H/W/D/bottom/top curves (10115a3b).
//! Shield common header, lifetime8/deform9/UV10/alpha11/material12/UVscale13..14/
//! UVspeed15..16/reserved17/frequency18/layers19/mech20, colour+offset curves21+.
//! Trail common header, lifetime8/mode9/material10/capacity11/cadence12/
//! curve frequency13/thrust14/fluctuation15, colour/X/Y curves16+.
//! All four native SetDuration slots are no-ops. Scatter forwards graceful stop;
//! Shield finishes two seconds later; Grid ignores it; Trail finishes immediately.
use super::{Binding, EffectConfig, Renderer, Template};
use anyhow::{ensure, Result};
use ao_formats::weather::R250;
use ao_scene::{Blend, Vertex};

// GC101161a1/1011634c/1011623a: zero knots => one/white, last knot fallback.
type Knot = (f32, u32);
#[derive(Clone)]
struct Curve { knots:Vec<Knot> }
impl Curve {
    fn parse(t:&Template,at:&mut usize,color:bool)->Result<Self> {
        let count=t.words.get(*at).copied().unwrap_or(0) as usize;*at+=1;
        ensure!(count<=t.words.len().saturating_sub(*at)/2,"truncated native effect curve");
        let mut knots=Vec::with_capacity(count);
        for _ in 0..count {let phase=t.float(*at)?;let value=t.word(*at+1)?;if !color {t.float(*at+1)?;}knots.push((phase,value));*at+=2;}
        Ok(Self {knots})
    }
    fn segment(&self,phase:f32)->Option<(Knot,Knot,f32)> {
        self.knots.windows(2).find(|w|w[0].0<=phase && phase<w[1].0).map(|w|(w[0],w[1],(phase-w[0].0)/(w[1].0-w[0].0)))
    }
    fn scalar(&self,phase:f32)->f32 {
        if let Some((a,b,u))=self.segment(phase) {(1.0-u)*f32::from_bits(a.1)+u*f32::from_bits(b.1)}
        else {self.knots.last().map(|k|f32::from_bits(k.1)).unwrap_or(1.0)}
    }
    fn color(&self,phase:f32)->[f32;4] {
        if let Some((a,b,u))=self.segment(phase) {mix(rgba(a.1),rgba(b.1),u)}
        else {rgba(self.knots.last().map(|k|k.1).unwrap_or(u32::MAX))}
    }
}
fn rgba(word:u32)->[f32;4] {let [a,r,g,b]=word.to_be_bytes();[r as f32/255.0,g as f32/255.0,b as f32/255.0,a as f32/255.0]}
fn mix(a:[f32;4],b:[f32;4],u:f32)->[f32;4] {std::array::from_fn(|i|a[i]+(b[i]-a[i])*u)}
fn render(mut c:[f32;4])->[f32;4] {for v in &mut c[..3] {*v=v.clamp(0.0,1.0).powf(2.2);}c[3]=c[3].clamp(0.0,1.0);c}
use glam::{Mat4, Vec3};

struct ScatterSlot { delay: f32, fired: bool, child: u32 }
pub(super) struct ScatterEffect {
    template: Template,
    source: Mat4,
    target: Vec3,
    config: EffectConfig,
    slots: Vec<ScatterSlot>,
    elapsed: f32,
    cycle_start: f32,
    terminating: bool,
}
impl ScatterEffect {
    pub(super) fn new(t:&Template, source:Mat4, target:Vec3, config:EffectConfig, random:&mut R250)->Result<Self> {
        ensure!(t.kind==3029,"not native Scatter");
        t.word(10)?;
        ensure!(t.word(4)?==0 && t.word(5)?<=1 && t.word(0)?&0x1000==0,"Scatter requires native ring/terrain placement outside the persistent binding");
        for i in [1,2,3,7,8,9] {t.float(i)?;}
        let count=t.word(6)? as usize;
        ensure!(count<=u16::MAX as usize,"Scatter capacity exceeds native index range");
        let duration=t.float(7)?;
        let slots=(0..count).map(|i|ScatterSlot {delay:if t.words[5]==0 {super::random_fraction(random)*duration}else{duration*i as f32/count as f32},fired:false,child:0}).collect();
        // SetDuration, SetColor and SetNumRepetitions are base no-ops in this vtable.
        Ok(Self {template:t.clone(),source,target,config,slots,elapsed:0.0,cycle_start:0.0,terminating:false})
    }
    pub(super) fn update_source(&mut self,source:Mat4) {self.source=source;}
    pub(super) fn graceful(&mut self,renderer:&mut Renderer)->Result<()> {
        self.template.words[0]&=!0x400;
        for slot in &self.slots {if slot.child!=0 {renderer.terminate_gracefully(slot.child);}}
        self.terminating=true;
        Ok(())
    }
    pub(super) fn cancel(&mut self,renderer:&mut Renderer)->Result<()> {
        for slot in &mut self.slots {if slot.child!=0 {renderer.delete(slot.child);slot.child=0;}}
        Ok(())
    }
    pub(super) fn frame(&mut self,dt:f32,renderer:&mut Renderer)->Result<bool> {
        self.elapsed+=dt;
        let t=&self.template;
        if self.terminating && t.words[0]&0x4000!=0 {return Ok(false);}
        let mut alive=false;
        for slot in &mut self.slots {
            if slot.child!=0 && !renderer.is_active(slot.child) {slot.child=0;}
            if slot.child==0 && !slot.fired {
                if slot.delay>self.elapsed-self.cycle_start {alive=true;continue;}
                // Native samples three values even when the authored radius is zero.
                let direction=Vec3::new(super::random_fraction(&mut renderer.random)*2.0-1.0,super::random_fraction(&mut renderer.random)*2.0-1.0,-(super::random_fraction(&mut renderer.random)*2.0-1.0));
                let offset=direction*t.float(8)?;
                let mut source=self.source;
                let mut config=self.config;
                if t.words[0]&0x2000==0 {
                    source=Mat4::from_translation(self.source.w_axis.truncate()+offset+Vec3::new(t.float(1)?,t.float(2)?,-t.float(3)?));
                    config.source_identity=None;
                }
                let id=t.word(10)? as i32;
                if id!=0 {slot.child=renderer.spawn_configured(Binding {group:0,attractor:0,effect:id,note:0,color:0},source,self.target,config)?;}
                slot.fired=true;
            }
            alive|=slot.child!=0;
        }
        if !alive && t.words[0]&0x400!=0 {
            for slot in &mut self.slots {slot.fired=false;}
            self.cycle_start=self.elapsed;
            alive=true;
        }
        Ok(alive)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn native_scatter_ignores_external_duration_and_schedules_authored_children() {
        let t=Template {kind:3029,words:vec![0x2000,0,0,0,0,1,4,3.0f32.to_bits(),0,0,72266]};
        let mut random=R250::new(0xe6f1);
        let effect=ScatterEffect::new(&t,Mat4::IDENTITY,Vec3::ZERO,EffectConfig {duration:Some(100.0),..Default::default()},&mut random).unwrap();
        assert_eq!(effect.slots.iter().map(|s|s.delay).collect::<Vec<_>>(),[0.0,0.75,1.5,2.25]);
        assert_eq!(effect.template.words[10],72266);
    }
    #[test] fn native_curve_knots_fan_lifetime_and_trail_termination() {
        let curve=Curve {knots:vec![(0.0,0.0f32.to_bits()),(0.5,2.0f32.to_bits()),(1.0,4.0f32.to_bits())]};
        assert_eq!(curve.scalar(0.0),0.0);assert_eq!(curve.scalar(0.5),2.0);assert_eq!(curve.scalar(-1.0),4.0);
        let mut gc=R250::new(0xe6f1);let mut ds=R250::new(0xe6f1);let mut crt=ao_formats::character::CrtRand::new(1);
        let mut words=vec![0;22];words[8]=3.0f32.to_bits();words[9]=100;words[14]=1;words[15]=1;words[16]=1;
        let mut grid=GridEffect::new(&Template {kind:3038,words},Mat4::IDENTITY,Mat4::IDENTITY,0,&mut gc,&mut ds,&mut crt).unwrap();
        grid.configure(EffectConfig {duration:Some(100.0),..Default::default()}).unwrap();
        assert_eq!(grid.models()[0].1.len(),36);
        assert_eq!(grid.vertices(1.0,Vec3::Z*10.0,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt).unwrap().unwrap()[0].len(),15);
        assert!(grid.vertices(3.0,Vec3::ZERO,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt).unwrap().is_none());
        let mut words=vec![0;19];words[8]=(-1.0f32).to_bits();words[10]=46;words[11]=8;words[13]=0.5f32.to_bits();
        let mut trail=TrailEffect::new(&Template {kind:3039,words},Mat4::IDENTITY,Mat4::IDENTITY,0,&mut gc,&mut ds,&mut crt).unwrap();
        let n=trail.models()[0].2;
        assert_eq!(trail.vertices(1.0,Vec3::ZERO,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt).unwrap().unwrap()[0].len(),n);
        trail.graceful();
        assert!(trail.vertices(2.0,Vec3::ZERO,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt).unwrap().is_none());
    }
    #[test] fn native_shield_duplicates_posed_surface_layers_and_has_two_second_finish() {
        let mut words=vec![0;27];words[8]=20.0f32.to_bits();words[12]=77;words[19]=2;words[20]=u32::MAX;
        words[21]=1;words[23]=u32::MAX;words[24]=1;words[26]=0.1f32.to_bits();
        let mut shield=ShieldEffect::new(&Template {kind:3034,words},EffectConfig {duration:Some(100.0),..Default::default()}).unwrap();
        let vertices=shield.vertices(&[Vertex {pos:[1.0,0.0,0.0],normal:[1.0,0.0,0.0],..Default::default()}],1.0);
        assert_eq!(vertices.len(),2);assert!((vertices[0].pos[0]-1.1).abs()<1e-6);assert!((vertices[1].pos[0]-1.155).abs()<1e-6);
        shield.template.words[0]|=0x20000;shield.stop_surface();
        assert!(shield.vertices(&[Vertex::default()],1.0).is_empty());
        assert!(shield.frame(18.1));assert!(shield.stop.is_some());assert!(shield.frame(2.0));assert!(!shield.frame(0.01));
    }
}

/// GC10115a3b/10115d41; DS1002f86d/1002f69e/100302ad.
pub(super) struct GridEffect {template:Template,source:Mat4,position:Vec3,curves:[Curve;5],duration:f32,previous:f32,angle:f32}
impl GridEffect {
    pub(super) fn new(t:&Template,source:Mat4,_target:Mat4,_color:u32,gc:&mut R250,_ds:&mut R250,_crt:&mut ao_formats::character::CrtRand)->Result<Self> {
        ensure!(t.kind==3038,"not native VolGrid");t.word(16)?;
        for i in [1,2,3,4,5,6,8,12,13] {t.float(i)?;}
        ensure!(t.float(8)?>0.0,"invalid VolGrid duration");
        ensure!(t.words[14..17].iter().map(|n|u64::from(*n)).sum::<u64>()<=u16::MAX as u64/5,"VolGrid index overflow");
        super::materials::MATERIALS.get(t.words[9] as usize).ok_or_else(||anyhow::anyhow!("unknown VolGrid material"))?;
        let mut at=17;
        let curves=[Curve::parse(t,&mut at,false)?,Curve::parse(t,&mut at,false)?,Curve::parse(t,&mut at,false)?,Curve::parse(t,&mut at,true)?,Curve::parse(t,&mut at,true)?];
        let source=super::sprites::connector(t,source)?;
        let angle=if t.words[0]&0x1000!=0 {super::random_fraction(gc)*std::f32::consts::TAU-std::f32::consts::PI}else{0.0};
        Ok(Self {template:t.clone(),source,position:source.w_axis.truncate(),curves,duration:t.float(8)?,previous:0.0,angle})
    }
    // Native SetDuration and TerminateGracefully are no-ops (100793e8/10115963).
    pub(super) fn configure(&mut self,_config:EffectConfig)->Result<()> {Ok(())}
    pub(super) fn update_source(&mut self,source:Mat4)->Result<()> {self.source=super::sprites::connector(&self.template,source)?;Ok(())}
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {
        let n=self.template.words[14..17].iter().sum::<u32>();
        vec![(Some(self.template.words[9] as usize),(0..n).flat_map(|i| {let b=i*5;[b,b+2,b+1,b,b+3,b+2,b,b+4,b+3,b,b+1,b+4]}).collect(),n as usize*5)]
    }
    pub(super) fn blends(&self)->Vec<Blend> {vec![Blend::Additive]}
    #[allow(clippy::too_many_arguments)]
    pub(super) fn vertices(&mut self,time:f32,camera:Vec3,_right:Vec3,_up:Vec3,_gc:&mut R250,_ds:&mut R250,_crt:&mut ao_formats::character::CrtRand)->Result<Option<Vec<Vec<Vertex>>>> {
        if time>=self.duration {return Ok(None);}
        let dt=(time-self.previous).max(0.0);self.previous=time;
        let t=&self.template;
        if t.words[0]&0x800!=0 {self.position=self.source.w_axis.truncate();}
        self.angle+=t.float(13)?*dt;
        let phase=(time%self.duration)/self.duration;
        let h=self.curves[0].scalar(phase);let w=self.curves[1].scalar(phase);let d=self.curves[2].scalar(phase);
        let bottom=self.curves[3].color(phase);let top=self.curves[4].color(phase);
        let rotation=Mat4::from_cols(self.source.x_axis.truncate().normalize_or_zero().extend(0.0),self.source.y_axis.truncate().normalize_or_zero().extend(0.0),self.source.z_axis.truncate().normalize_or_zero().extend(0.0),self.position.extend(1.0))*Mat4::from_rotation_y(-self.angle);
        let view=rotation.inverse().transform_vector3((camera-self.position).normalize_or_zero());
        let mut vertices=Vec::with_capacity(t.words[14..17].iter().sum::<u32>() as usize*5);
        // Retail order is X, Z, Y; each plane is a center plus four fan corners.
        for axis in [0,2,1] {
            let count=t.words[14+axis];
            for i in 0..count {
                let u=i as f32/count as f32;
                let points=match axis {
                    0=>{let x=u*w-w*0.5;[Vec3::new(x,h*0.5,0.0),Vec3::new(x,0.0,d*0.5),Vec3::new(x,0.0,-w*0.5),Vec3::new(x,h,-d*0.5),Vec3::new(x,h,d*0.5)]},
                    2=>{let z=-(u*w-w*0.5);[Vec3::new(0.0,h*0.5,z),Vec3::new(-w*0.5,0.0,z),Vec3::new(-d*0.5,h,z),Vec3::new(d*0.5,h,z),Vec3::new(w*0.5,0.0,z)]},
                    _=>{let y=h*u;[Vec3::new(0.0,y,0.0),Vec3::new(-w*0.5,y,-w*0.5),Vec3::new(-w*0.5,y,w*0.5),Vec3::new(w*0.5,y,w*0.5),Vec3::new(w*0.5,y,-w*0.5)]}
                };
                let colors=match axis {0=>[mix(bottom,top,0.5),bottom,bottom,top,top],2=>[mix(bottom,top,0.5),bottom,top,top,bottom],_=>[mix(bottom,top,u);5]};
                let uv=if axis==1 || t.words[0]&0x400!=0 {
                    if axis==0 {[[0.5,0.5],[0.0,0.0],[1.0,0.0],[1.0,1.0],[0.0,1.0]]}
                    else {[[0.5,0.5],[0.0,0.0],[0.0,1.0],[1.0,1.0],[1.0,0.0]]}
                }else if axis==0 {[[0.5,u],[0.0,u],[1.0,u],[1.0,u],[0.0,u]]}else{[[0.5,u],[0.0,u],[0.0,u],[1.0,u],[1.0,u]]};
                let normal=(points[1]-points[0]).normalize_or_zero().cross((points[2]-points[0]).normalize_or_zero());
                let fade=if t.words[0]&0x4000!=0 && view!=Vec3::ZERO && normal!=Vec3::ZERO {normal.dot(view).abs()}else{1.0};
                for j in 0..5 {let mut color=colors[j];color[3]=(color[3]*255.0*fade).trunc()/255.0;vertices.push(Vertex {pos:rotation.transform_point3(points[j]).to_array(),color:render(color),uv:uv[j],..Default::default()});}
            }
        }
        Ok(Some(vec![vertices]))
    }
}

/// Shield2 duplicates the live posed CAT surface, not its bounding box.
/// GC10110f3a/10110bbf/10111147; DS1001dc74/1001d569.
pub(super) struct ShieldEffect {template:Template,color:Curve,offset:Curve,duration:f32,elapsed:f32,stop:Option<f32>,surface_stopped:bool}
impl ShieldEffect {
    pub(super) fn new(t:&Template,_config:EffectConfig)->Result<Self> {
        ensure!(t.kind==3034,"not native Shield2");t.word(20)?;
        for i in [8,13,14,15,16,17,18] {t.float(i)?;}
        ensure!(t.words[9]==0 && t.words[10]==0 && t.words[11]<=2,"Shield2 requires native non-persistent deformation/UV mode");
        ensure!(t.words[20]==u32::MAX,"Shield2 requires native mech resource rather than CAT surface");
        let mut at=21;let color=Curve::parse(t,&mut at,true)?;let offset=Curve::parse(t,&mut at,false)?;
        // Slot9 SetDuration is a native no-op. Slot8 SetColor is not used by this path.
        Ok(Self {template:t.clone(),color,offset,duration:t.float(8)?,elapsed:0.0,stop:None,surface_stopped:false})
    }
    pub(super) fn graceful(&mut self) {if self.stop.is_none() {self.stop=Some(self.elapsed);}}
    pub(super) fn frame(&mut self,dt:f32)->bool {
        self.elapsed+=dt;
        if self.stop.is_some_and(|s|self.elapsed-s>2.0) {return false;}
        if self.duration>0.0 && self.elapsed>self.duration-2.0 {self.graceful();}
        true
    }
    pub(super) fn material(&self)->usize {self.template.words[12] as usize}
    pub(super) fn blend(&self)->Blend {if self.template.words[0]&0x400!=0 {Blend::Additive}else{Blend::AlphaBlend}}
    pub(super) fn layer_count(&self)->usize {self.template.words[19] as usize}
    /// GC10111147 second CAT callback -> DS1001dffe Stop; flags20000 hide it.
    pub(super) fn stop_surface(&mut self) {self.surface_stopped=true;}
    pub(super) fn vertices(&self,source:&[Vertex],body_scale:f32)->Vec<Vertex> {
        let t=&self.template;let phase=self.elapsed/self.duration;
        if self.surface_stopped && t.words[0]&0x20000!=0 {return Vec::new();}
        if t.words[0]&0x40000!=0 && source.len()>1000 {return Vec::new();}
        let offset=self.offset.scalar(phase);let color=self.color.color(phase);
        let frequency=f32::from_bits(t.words[18]);
        let u=f32::from_bits(t.words[13]);let vv=f32::from_bits(t.words[14]);
        let du=f32::from_bits(t.words[15])*phase;let dv=f32::from_bits(t.words[16])*phase;
        let surface=source.iter().map(|v| {
            let mut out=*v;let p=Vec3::from_array(v.pos);let n=Vec3::from_array(v.normal);
            out.pos=((p+n*offset)*body_scale).to_array();
            out.uv=[(v.uv[0]+du)*u,(v.uv[1]+dv)*vv];
            let z=-p.z;let f=phase*frequency;
            let mut alpha=1.0;
            if t.words[11]!=0 {
                alpha=((p.y*2.5+p.x*4.0+z*3.0+f*1.1)*2.5).cos()*((p.y*3.0+p.x*2.5+z*4.0+f)*2.3).sin()*((p.y*4.0+p.x*3.0+z*1.5+f*1.2)*1.5).sin();
                if t.words[11]==2 {alpha*=phase*1.5;}
            }
            let mut color=color;color[3]=(color[3]*alpha*255.0).trunc().clamp(0.0,255.0)/255.0;
            out.color=render(color);out
        }).collect::<Vec<_>>();
        let layers=self.layer_count();
        if layers==1 {return surface;}
        let mut out=Vec::with_capacity(source.len()*layers);
        for layer in 0..layers {
            let expand=layer as f32/layers as f32*offset;
            for vertex in &surface {
                let mut v=*vertex;
                let mut p=Vec3::from_array(v.pos)*(1.0+expand);
                if t.words[0]&0x1000!=0 {p.y-=expand;}
                v.pos=p.to_array();out.push(v);
            }
        }
        out
    }
}

#[derive(Clone,Copy)]
struct TrailSample {matrix:Mat4,color:[f32;4]}
/// GC10114c39/101150b4/10114bae; DS1002c7bd/1002c918/1002d169.
pub(super) struct TrailEffect {template:Template,source:Mat4,origin:TrailSample,samples:Vec<TrailSample>,head:usize,count:usize,curves:[Curve;3],previous:f32,emission:f32,done:bool}
impl TrailEffect {
    pub(super) fn new(t:&Template,source:Mat4,_target:Mat4,_color:u32,_gc:&mut R250,_ds:&mut R250,_crt:&mut ao_formats::character::CrtRand)->Result<Self> {
        ensure!(t.kind==3039,"not native Trail2");t.word(15)?;
        ensure!((1..=4096).contains(&t.words[11]),"invalid Trail2 sample capacity");
        for i in [1,2,3,4,5,6,8,12,13,14,15] {t.float(i)?;}
        super::materials::MATERIALS.get(t.words[10] as usize).ok_or_else(||anyhow::anyhow!("unknown Trail2 material"))?;
        let mut at=16;let curves=[Curve::parse(t,&mut at,true)?,Curve::parse(t,&mut at,false)?,Curve::parse(t,&mut at,false)?];
        let source=super::sprites::connector(t,source)?;
        let origin=TrailSample {matrix:source,color:[0.0;4]};
        Ok(Self {template:t.clone(),source,origin,samples:vec![origin;t.words[11] as usize],head:0,count:0,curves,previous:0.0,emission:0.0,done:false})
    }
    pub(super) fn configure(&mut self,_config:EffectConfig)->Result<()> {Ok(())}
    pub(super) fn update_source(&mut self,source:Mat4)->Result<()> {self.source=super::sprites::connector(&self.template,source)?;Ok(())}
    pub(super) fn graceful(&mut self) {self.done=true;}
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {
        let n=self.samples.len()+1;
        let indices=(0..4u32).flat_map(|strip|(0..n as u32-1).flat_map(move |i|{let b=strip*n as u32*2+i*2;[b,b+1,b+2,b+1,b+3,b+2]})).collect();
        vec![(Some(self.template.words[10] as usize),indices,n*8)]
    }
    pub(super) fn blends(&self)->Vec<Blend> {vec![Blend::Additive]}
    fn sample(&mut self,sample:TrailSample) {
        self.samples[self.head]=sample;self.head+=1;
        if self.count<self.head {self.count+=1;}
        if self.head==self.samples.len() {self.head=0;}
        let remaining=self.samples.len()-self.count;
        if remaining!=0 {for old in &mut self.samples[..remaining] {*old=sample;}}
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn vertices(&mut self,time:f32,_camera:Vec3,_right:Vec3,_up:Vec3,_gc:&mut R250,ds:&mut R250,_crt:&mut ao_formats::character::CrtRand)->Result<Option<Vec<Vec<Vertex>>>> {
        if self.done {return Ok(None);}
        let dt=(time-self.previous).max(0.0);self.previous=time;
        let phase=(time*f32::from_bits(self.template.words[13]))%1.0;
        let mut matrix=self.source;
        matrix.x_axis*=self.curves[1].scalar(phase);matrix.y_axis*=self.curves[2].scalar(phase);
        let movement=matrix.w_axis.truncate()-self.origin.matrix.w_axis.truncate();
        let mut color=self.curves[0].color(phase);
        let mut emit=true;
        if self.template.words[0]&0x800!=0 {
            if movement==Vec3::ZERO {emit=false;}
            else if movement.normalize().dot(-self.source.z_axis.truncate().normalize_or_zero())<=-0.75 {color=[0.0;4];}
        }
        if emit {
            self.origin=TrailSample {matrix,color};
            let period=f32::from_bits(self.template.words[12]);
            if period<=0.0 {self.sample(self.origin);}else{
                self.emission+=dt;
                while self.emission>=period {self.sample(self.origin);self.emission-=period;}
            }
        }
        if self.template.words[0]&0x1000!=0 && dt!=0.0 {
            let thrust=(super::random_fraction(ds)*f32::from_bits(self.template.words[15])+f32::from_bits(self.template.words[14]))*dt;
            let displacement=-self.origin.matrix.z_axis.truncate()*thrust;
            for sample in &mut self.samples[..self.count] {sample.matrix.w_axis+=displacement.extend(0.0);}
        }
        let n=self.samples.len()+1;
        let mut vertices=vec![Vertex::default();n*8];
        let active=self.count+1;
        // DS1002c918 leaves the final first-strip colour live for the other strips.
        let last_color=if self.count<self.samples.len()-1 {self.samples[(self.head+self.count)%self.samples.len()].color}else{self.origin.color};
        for strip in 0..4 {
            for i in 0..active {
                let sample=if i<self.samples.len()-1 {self.samples[(self.head+i)%self.samples.len()]}else{self.origin};
                let m=sample.matrix;let p=m.w_axis.truncate();let x=m.x_axis.truncate();let y=m.y_axis.truncate();
                let offsets=match strip {0=>[y,-y],1=>[-x,x],2=>[-x+y,x-y],_=>[x+y,-x-y]};
                let color=render(if strip==0 {sample.color}else{last_color});
                for side in 0..2 {
                    let u=if self.count==0 {0.0}else{i as f32/self.count as f32};
                    let uv=if self.template.words[0]&0x400==0 {[u,side as f32]}else{[side as f32,u]};
                    vertices[strip*n*2+i*2+side]=Vertex {pos:(p+offsets[side]).to_array(),uv,color,..Default::default()};
                }
            }
            // Fixed uploaded capacity; unused strip segments must be degenerate,
            // not a visible triangle from the live trail back to world origin.
            for i in active..n {
                for side in 0..2 {
                    let mut vertex=vertices[strip*n*2+(active-1)*2+side];
                    vertex.color=[0.0;4];
                    vertices[strip*n*2+i*2+side]=vertex;
                }
            }
        }
        Ok(Some(vec![vertices]))
    }
}
