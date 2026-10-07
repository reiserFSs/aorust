//! Native Electra 2006: GC100d9262/100d9388/100d988d/100d9135;
//! DS100118d3. All simulation below uses native coordinates, reflected at draw.
use super::{buff200x::{authored_color, circle, render_color}, materials, sprites, EffectConfig, Template};
use anyhow::{ensure, Context, Result};
use ao_formats::{character::CrtRand, weather::R250};
use ao_scene::{Blend, Vertex};
use glam::{Mat3, Mat4, Quat, Vec3};
const ANCHORS:[i32;10]=[1011,1013,1012,1014,1007,1009,1008,1010,1000,1004];
const PAIRS:[(usize,usize);6]=[(0,1),(2,3),(4,5),(6,7),(4,6),(8,9)];
const AXES:[usize;6]=[2,2,1,1,0,1];
fn reflect(v:Vec3)->Vec3 {Vec3::new(v.x,v.y,-v.z)}
fn perpendicular(v:Vec3)->Vec3 {
    let mut p=Vec3::new(v.y-v.z,v.z-v.x,v.x-v.y);
    if p.length_squared()==0.0 {p=Vec3::new(v.z+v.y,v.z-v.x,-v.x-v.y);}
    if p.length_squared()==0.0 {Vec3::X}else{v.cross(p).normalize()}
}
fn segment_basis(axis:Vec3,delta:Vec3)->Mat3 {
    let cross=axis.cross(delta);
    if cross==Vec3::ZERO && axis==Vec3::ZERO {return Mat3::IDENTITY;}
    let x=cross.normalize_or_zero();let z=axis.normalize_or_zero();
    Mat3::from_cols(x,z.cross(x),z)
}
#[derive(Clone,Copy)]
struct Slot {end:f32,point:Vec3,basis:Mat3,segment:usize,flip:bool,visible:bool}
impl Default for Slot {fn default()->Self {Self {end:-100.0,point:Vec3::ZERO,basis:Mat3::IDENTITY,segment:0,flip:false,visible:false}}}
pub(super) struct ElectraEffect {template:Template,source:Mat4,slots:[Slot;64],anchors:[Option<Mat4>;10],duration:f32,life:f32,last_tick:i32,terminating:bool,radii:[f32;12]}
impl ElectraEffect {
    pub(super) fn new(t:&Template,source:Mat4,_target:Mat4,_color:u32,_gc:&mut R250,_ds:&mut R250,_crt:&mut CrtRand)->Result<Self> {
        ensure!(t.kind==2006 && t.word(10)?<3,"invalid native Electra mode");
        for i in (1..=6).chain(11..=29) {t.float(i)?;}
        ensure!(t.word(10)?!=0 || t.word(31)?<=6,"invalid Electra body segment");
        let life=t.word(30)? as i32 as f32/1000.0;ensure!(life>0.0,"invalid Electra lifetime");
        materials::MATERIALS.get(t.word(9)? as usize).context("unknown Electra material")?;
        Ok(Self {template:t.clone(),source:sprites::connector(t,source)?,slots:[Slot::default();64],anchors:[None;10],duration:t.float(26)?,life,last_tick:0,terminating:false,radii:RADII[0]})
    }
    pub(super) fn configure(&mut self,c:EffectConfig)->Result<()> {
        if let Some(d)=c.duration {self.duration=d;}
        if let Some([breed,sex,fat,_])=c.source_appearance {
            if (1..=4).contains(&breed) {let index=(breed-1)*6+if sex==3 {3}else{0}+fat;ensure!(index>=0 && (index as usize)<RADII.len(),"invalid Electra body profile");self.radii=RADII[index as usize];}
        }
        Ok(())
    }
    pub(super) fn anchor_ids(&self)->&[i32] {if self.template.words[10]==0 {&ANCHORS}else{&[]}}
    pub(super) fn update_anchor(&mut self,id:i32,m:Option<Mat4>) {if let Some(i)=ANCHORS.iter().position(|&a|a==id) {self.anchors[i]=m;}}
    pub(super) fn update_source(&mut self,m:Mat4)->Result<()> {self.source=sprites::connector(&self.template,m)?;Ok(())}
    pub(super) fn terminate_gracefully(&mut self) {self.terminating=true;}
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {vec![(Some(self.template.words[9] as usize),(0..64).flat_map(|i|[i*4,i*4+1,i*4+2,i*4+1,i*4+2,i*4+3]).collect(),256)]}
    pub(super) fn blends(&self)->Vec<Blend> {vec![Blend::Additive]}
    #[allow(clippy::too_many_arguments)]
    pub(super) fn vertices(&mut self,time:f32,_camera:Vec3,_right:Vec3,_up:Vec3,_gc:&mut R250,_ds:&mut R250,crt:&mut CrtRand)->Result<Option<Vec<Vec<Vertex>>>> {
        let mode=self.template.words[10];
        if mode<2 && self.duration>=0.0 && time>=self.duration && !self.terminating {self.terminating=true;self.duration+=self.life;}
        let tick=(28.0/self.life*time) as i32;let mut budget=(tick-self.last_tick).clamp(0,2);self.last_tick=tick;
        let axes=[reflect(self.source.x_axis.truncate()),reflect(self.source.y_axis.truncate()),reflect(self.source.z_axis.truncate())];
        let origin=reflect(self.source.w_axis.truncate());
        let mut segments=[None;6];
        if mode==0 {for (i,&(a,b)) in PAIRS.iter().enumerate() {if let (Some(a),Some(b))=(self.anchors[a],self.anchors[b]) {let start=reflect(a.w_axis.truncate());let delta=start-reflect(b.w_axis.truncate());segments[i]=Some((start,delta.length(),segment_basis(axes[AXES[i]],delta)));}}}
        let width=self.template.float(28)?;let radius=self.template.float(29)?;
        let material=materials::MATERIALS[self.template.words[9] as usize];
        let mut out=vec![Vertex {color:[0.0;4],..Default::default()};256];
        for (i,s) in self.slots.iter_mut().enumerate() {
            if s.end<=time {
                s.visible=false;
                if budget==0 || self.terminating {continue;}
                if mode==0 && segments.iter().any(Option::is_none) {continue;}
                if mode==2 {let p=reflect(circle(crt));s.point=Vec3::new(0.0,p.x,p.z);}
                else {
                    let mut p=if mode==0 {reflect(circle(crt))}else{sprites::sphere_point(crt)};
                    if mode==1 {p.y*=1.5;s.point=p*radius;p=p.normalize_or_zero();}
                    let angle=crt.rand() as f32/f32::from_bits(0x461c18cd);
                    let perpendicular=if mode==0 {Vec3::Y}else{perpendicular(s.point)};
                    let tangent=Quat::from_axis_angle(p,angle)*perpendicular;
                    s.basis=Mat3::from_cols(tangent,p,tangent.cross(p));
                    if mode==0 {s.point=p;s.point.y=crt.rand() as f32/32768.0;s.segment=crt.rand() as usize%6;let segment=self.template.word(31)?;if segment!=6 {s.segment=segment as usize;}}
                }
                s.end=time+self.life;s.flip=crt.rand()&1==1;budget-=1;
                // GC spawn branch does not draw until the next Process call.
                continue;
            }
            let phase=(self.life-(s.end-time))/self.life;
            let (center,mut a,b)=match mode {
                0=>{let Some((start,length,basis))=segments[s.segment] else {continue;};let r=if self.template.word(31)?==6 {self.radii[s.segment*2]+self.radii[s.segment*2+1]*s.point.y}else{width+radius*s.point.y}*length;let size=if r<=0.1 {r*3.0}else{0.3};(start+basis*Vec3::new(s.point.x*r,s.point.y*length,s.point.z*r),(basis*s.basis).x_axis*size,(basis*s.basis).z_axis*size)},
                1=>(origin+s.point,s.basis.x_axis*width,s.basis.z_axis*width),
                _=>{let a=-axes[2]*width;(origin-a*0.5,a,(axes[0]*s.point.y+axes[1]*s.point.z)*radius)}
            };
            if mode<2 && s.flip {a = -a;}
            let frame=(phase*if mode==2 {32.0}else{16.0}) as u32;
            let u=(frame%material.1) as f32/material.1 as f32;let v=(frame/material.1) as f32/material.2 as f32;
            let du=1.0/material.1 as f32;let dv=1.0/material.2 as f32;
            let color=render_color(authored_color(&self.template,if mode==2 {0.0}else{time/self.duration}));
            for (j,(p,uv)) in [(center-a*0.5-b*0.5,[u,v+dv]),(center+a*0.5-b*0.5,[u+du,v+dv]),(center-a*0.5+b*0.5,[u,v]),(center+a*0.5+b*0.5,[u+du,v])].into_iter().enumerate() {out[i*4+j]=Vertex {pos:reflect(p).to_array(),color,uv,..Default::default()};}
            s.visible=true;
        }
        if self.terminating && self.slots.iter().all(|s|s.end<=time) {Ok(None)}else{Ok(Some(vec![out]))}
    }
}
// GC1016b788: six linear radius pairs per breed/sex/body-shape profile.
const RADII:[[f32;12];21]=[
    [0.15,-0.05,0.15,-0.05,0.2,0.05,0.2,0.05,0.2,0.0,0.3,0.1],
    [0.15,0.0,0.15,0.0,0.2,0.05,0.2,0.05,0.2,0.0,0.3,0.1],
    [0.3,-0.1,0.3,-0.1,0.25,0.0,0.25,0.0,0.25,0.0,0.5,0.0],
    [0.13,0.0,0.13,0.0,0.1,0.0,0.1,0.0,0.15,0.0,0.2,0.15],
    [0.14,0.0,0.14,0.0,0.15,0.0,0.15,0.0,0.15,0.0,0.25,0.15],
    [0.2,-0.04,0.2,-0.04,0.15,0.0,0.15,0.0,0.17,0.0,0.3,0.1],
    [0.11,0.0,0.11,0.0,0.15,0.0,0.15,0.0,0.16,0.0,0.2,0.2],
    [0.12,0.0,0.12,0.0,0.16,0.0,0.16,0.0,0.17,0.0,0.2,0.21],
    [0.13,0.0,0.13,0.0,0.17,0.0,0.17,0.0,0.18,0.0,0.2,0.21],
    [0.11,0.0,0.11,0.0,0.1,0.0,0.1,0.0,0.16,0.0,0.2,0.1],
    [0.12,0.0,0.12,0.0,0.11,0.0,0.11,0.0,0.17,0.0,0.2,0.1],
    [0.13,0.0,0.13,0.0,0.12,0.0,0.12,0.0,0.18,0.0,0.25,0.1],
    [0.13,0.0,0.13,0.0,0.15,0.0,0.15,0.0,0.25,0.0,0.3,0.1],
    [0.2,-0.05,0.2,-0.05,0.18,0.0,0.18,0.0,0.26,0.0,0.3,0.15],
    [0.3,-0.14,0.3,-0.14,0.25,-0.05,0.25,-0.05,0.27,0.0,0.55,0.0],
    [0.13,-0.02,0.13,-0.02,0.11,0.0,0.11,0.0,0.12,0.0,0.27,0.0],
    [0.14,-0.03,0.14,-0.03,0.12,0.0,0.12,0.0,0.13,0.0,0.28,0.0],
    [0.24,-0.08,0.24,-0.08,0.15,0.0,0.15,0.0,0.15,0.0,0.35,0.0],
    [0.4,0.0,0.4,0.0,0.4,0.0,0.4,0.0,0.3,0.0,0.4,0.0],
    [0.25,0.15,0.25,0.15,0.2,0.05,0.2,0.05,0.25,0.0,0.35,0.25],
    [0.4,0.0,0.4,0.0,0.4,0.0,0.4,0.0,0.3,0.0,0.4,0.0],
];
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn absent_electra_segment_defaults_to_zero() -> Result<()> {
        let mut words=vec![0;31];words[26]=(-1.0f32).to_bits();words[30]=500;
        let mut gc=R250::new(1);let mut ds=R250::new(2);let mut crt=CrtRand::new(3);
        let mut e=ElectraEffect::new(&Template {kind:2006,words},Mat4::IDENTITY,Mat4::IDENTITY,0,&mut gc,&mut ds,&mut crt)?;
        e.vertices(0.04,Vec3::ZERO,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt)?;
        assert!(e.slots.iter().filter(|s|s.end>0.04).all(|s|s.segment==0));
        e.vertices(0.1,Vec3::ZERO,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt)?;Ok(())
    }
    #[test] fn native_basis_and_drain() {
        assert_eq!(segment_basis(Vec3::Z,-Vec3::Y)*Vec3::Y,Vec3::Y);
        for mode in 0..3 {
            let mut words=vec![0;32];words[9]=2;words[10]=mode;words[26]=(-1.0f32).to_bits();words[28]=6.0f32.to_bits();words[29]=1.7f32.to_bits();words[30]=500;words[31]=6;
            let t=Template {kind:2006,words};let mut gc=R250::new(1);let mut ds=R250::new(1);let mut crt=CrtRand::new(1);
            let mut e=ElectraEffect::new(&t,Mat4::IDENTITY,Mat4::IDENTITY,0,&mut gc,&mut ds,&mut crt).unwrap();
            for (i,id) in ANCHORS.into_iter().enumerate() {e.update_anchor(id,Some(Mat4::from_translation(Vec3::new(i as f32,i as f32*0.3,0.0))));}
            e.vertices(0.04,Vec3::ZERO,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt).unwrap();
            assert_eq!(e.slots.iter().filter(|s|s.end>0.04).count(),2);
            e.terminate_gracefully();
            assert!(e.vertices(0.1,Vec3::ZERO,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt).unwrap().is_some());
            assert!(e.vertices(0.55,Vec3::ZERO,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt).unwrap().is_none());
        }
    }
}
