//! GC Suns: loader 100fcbf8, init 100fcd1e, process 100fbecf, graceful 100fcacb.
//! DS Sol 10020836: rotated camera quads, radius offset, SRCALPHA/ONE.
use super::{materials, quad, sprites, EffectConfig, Template};
use anyhow::{ensure, Context, Result};
use ao_formats::{character::CrtRand, weather::R250};
use ao_scene::{Blend, Vertex};
use glam::{Mat4, Vec3};

pub(super) fn authored_color(t: &Template, phase: f32) -> [f32; 4] {
    let f = |i| f32::from_bits(t.words.get(i).copied().unwrap_or(0));
    // CMS colour controls store ARGB floats, not packed words (10108089).
    [19, 20, 21, 18].map(|i| f(i) + (f(i + 4) - f(i)) * phase)
}
pub(super) fn render_color(c: [f32; 4]) -> [f32; 4] {
    let byte = |v: f32| ((v * 255.0 - 0.49999).round_ties_even() as i32 as u32) & 255;
    let b = c.map(byte);
    [(b[0] as f32 / 255.0).powf(2.2), (b[1] as f32 / 255.0).powf(2.2), (b[2] as f32 / 255.0).powf(2.2), b[3] as f32 / 255.0]
}
pub(super) fn circle(r: &mut CrtRand) -> Vec3 {
    loop {
        let x = r.rand() as f32 * (2.0 / 32768.0) - 1.0;
        let z = r.rand() as f32 * (2.0 / 32768.0) - 1.0;
        let len = x * x + z * z;
        if len > 0.0 && len < 1.0 { return Vec3::new(x, 0.0, -z) / len.sqrt(); }
    }
}
fn rgba(c: u32) -> [f32; 4] { let [a,r,g,b] = c.to_be_bytes(); [r,g,b,a].map(|v| v as f32 / 255.0) }
const SUN: [u32; 8] = [0xffffc0c0,0xffffff00,0xffff00ff,0xff00ffff,0xff0000ff,0xff00ff00,0xffff0000,0xffc0ffc0];
const ORBIT: [u32; 8] = [0xffffc000,0xff0040ff,0xffff0040,0xff00ffc0,0xffc000ff,0xff40ff00,0xffc00040,0xff40ffc0];
#[derive(Clone, Copy)]
struct Sprite { position: Vec3, size: f32, angle: f32, radius: f32, color: [f32;4], frame: i32, visible: bool }
impl Default for Sprite { fn default() -> Self { Self {position:Vec3::ZERO,size:0.0,angle:0.0,radius:0.0,color:[0.0;4],frame:0,visible:false} } }
pub(super) struct Suns {
    template: Template, source: Mat4, duration: f32,
    stop: bool, alive: bool, sprites: [Sprite;32], deadlines: [f32;32], origins: [Vec3;32],
    anchors: [Option<Vec3>;3], hit: [Vec3;2], emitted: i32,
}
impl Suns {
    pub(super) fn supports(kind: i32) -> bool { kind == 2005 }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(t: &Template, source: Mat4, target: Mat4, _color: u32, _gc: &mut R250, _ds: &mut R250, _crt: &mut CrtRand) -> Result<Self> {
        ensure!(Self::supports(t.kind), "not a native Suns template");
        t.word(31)?; ensure!(t.word(10)? < 5, "invalid Suns mode");
        materials::MATERIALS.get(t.word(9)? as usize).context("unknown Suns material")?;
        for i in (1..=6).chain(8..=8).chain(11..=29) { t.float(i)?; }
        let duration = t.float(26)?;
        let life = t.word(30)? as f32 / 1000.0;
        ensure!(t.word(10)? < 2 || t.word(10)? == 3 || life > 0.0, "invalid Suns particle lifetime");
        Ok(Self {template:t.clone(),source:sprites::connector(t,source)?,duration,stop:false,alive:true,sprites:[Sprite::default();32],deadlines:[-100.0;32],origins:[Vec3::ZERO;32],anchors:[None;3],hit:[source.w_axis.truncate(),target.w_axis.truncate()],emitted:0})
    }
    pub(super) fn configure(&mut self, c: EffectConfig) -> Result<()> {
        if let Some(d) = c.duration { ensure!(d.is_finite(), "nonfinite Suns duration"); self.duration=d; }
        if let Some(color)=c.start_color { for (i,v) in [19,20,21,18].into_iter().zip(color) {self.template.words[i]=v.to_bits();} }
        if let Some(color)=c.stop_color { for (i,v) in [23,24,25,22].into_iter().zip(color) {self.template.words[i]=v.to_bits();} }
        Ok(())
    }
    pub(super) fn update_source(&mut self, source: Mat4) -> Result<()> {self.source=sprites::connector(&self.template,source)?;Ok(()) }
    pub(super) fn required_attractors(&self) -> &'static [i32] {match self.template.words[10] {2=>&[1006,2000,2001],3=>&[1006],_=>&[]}}
    pub(super) fn set_attractor(&mut self, id: i32, matrix: Option<Mat4>) { if let Some(i)=[1006,2000,2001].iter().position(|v|*v==id) {self.anchors[i]=matrix.map(|m|m.w_axis.truncate());} }
    pub(super) fn update_hit(&mut self, start:Vec3,end:Vec3) {self.hit=[start,end];}
    pub(super) fn terminate_gracefully(&mut self) {self.stop=true;}
    pub(super) fn models(&self) -> Vec<(Option<usize>,Vec<u32>,usize)> {vec![(Some(self.template.words[9] as usize),(0..32).flat_map(|i|[i*4,i*4+2,i*4+3,i*4,i*4+3,i*4+1]).collect(),128)]}
    pub(super) fn blends(&self) -> Vec<Blend> {vec![Blend::Additive]}
    #[allow(clippy::too_many_arguments)]
    fn pair(&mut self,i:usize,position:Vec3,size:f32,angle:f32,second:f32,second_angle:f32,radius:f32,mask:u32) {
        self.sprites[i]=Sprite {position,size,angle,radius:0.0,color:rgba(ORBIT[i&7]&mask),frame:0,visible:true};
        self.sprites[i+1]=Sprite {position,size:second,angle:second_angle,radius,color:rgba(ORBIT[(i+1)&7]&mask),frame:0,visible:true};
    }
    // The authored orbit uses GC double 1016b578 = 6.28000020980835, not TAU.
    #[allow(clippy::approx_constant)]
    fn simulate(&mut self,time:f32,crt:&mut CrtRand) {
        let mode=self.template.words[10];let size=f32::from_bits(self.template.words[28]);let speed=f32::from_bits(self.template.words[29]);let p=time/self.duration;
        let centre=self.source.w_axis.truncate();
        match mode {
            0 => {let pulse=1.0-(2.0*p-1.0).powi(4);let pos=centre+Vec3::Y*2.0;for (i,color) in SUN.into_iter().enumerate() {self.sprites[i]=Sprite {position:pos,size:(i as f32*speed+size)*pulse,angle:time*0.5*(i as f32-4.0),radius:0.0,color:rgba(color&0xc0ffffff),frame:0,visible:true};}}
            1 => {
                let reverse=self.template.words[30]!=0;let mut q=if reverse {1.0-p}else{p};let remain=1.0-q;
                if reverse && q<0.2 {q=1.0-remain*(q*5.0).powi(27);}
                let mask=((255.0*(1.0-q*q)) as u32)<<24|0xffffff;
                let mut pulse=1.0-remain*remain;let mut a=size*pulse*0.9;let mut b=size*pulse*1.1;
                if self.template.words[31]!=0 {let extra=size*(1.0-q).powi(8);a+=extra;b+=extra;pulse=pulse.max(0.3);}
                for i in (0..18).step_by(2) {let angle=time*speed+i as f32*6.28*0.0555555;let pos=centre+Vec3::new(angle.sin(),(angle*3.0+time*4.0).sin()*0.1,-angle.cos())*pulse;self.pair(i,pos,a,time*2.0,b,time*(-1.5),pulse*0.05,mask);}
            }
            2 | 3 => {
                let head=self.anchors[0].unwrap_or(Vec3::ZERO);
                let pulse=if mode==2 {p}else{1.0-(2.0*p-1.0).powi(4)};
                let pos=if mode==2 {head+Vec3::Y*1.7}else{head};
                let a=size*pulse*0.9;let b=size*pulse*1.1;
                for i in (0..4).step_by(2) {self.pair(i,pos,a,time*1.3*(i as f32-1.5),b,time*(-1.5)*(i as f32-2.5),a*0.05,u32::MAX);}
                if mode==2 {
                    let life=self.template.words[30] as f32/1000.0;
                    // GC truncates total emissions, caps catch-up at two hand pairs per frame.
                    let total=(time*12.0/life) as i32;let mut count=(total-self.emitted).min(2);self.emitted=total;
                    for i in (4..32).step_by(2) {
                        if self.deadlines[i]<=time {if count>0 {let hand=(!(total-count)&1) as usize+1;self.origins[i]=self.anchors[hand].unwrap_or(Vec3::ZERO);self.deadlines[i]=time+life;count-=1;}else{self.deadlines[i]= -100.0;}}
                        else {let u=((life-(self.deadlines[i]-time))/life).powi(2);let point=self.origins[i].lerp(pos,u);let parity=(i&1) as f32-0.5;self.pair(i,point,speed*0.9,time*1.3*(parity-1.5),speed*1.1,time*(-1.5)*(parity-2.5),speed*0.9*0.05,u32::MAX);}
                    }
                }
            }
            4 => {
                let life=self.template.words[30] as f32/1000.0;let mut emissions=3;let mut live=0;
                for i in 0..32 {
                    if self.deadlines[i]<=time {
                        if emissions>0 && !self.stop {let u=crt.rand() as f32/32768.0;self.origins[i]=self.hit[0].lerp(self.hit[1],u);self.deadlines[i]=time+life;emissions-=1;live+=1;}
                        else {self.sprites[i].visible=false;}
                    } else {let u=(time+life-self.deadlines[i])/life;self.sprites[i]=Sprite {position:self.origins[i],size,angle:(i%7) as f32*0.3,radius:0.0,color:authored_color(&self.template,u),frame:15-(speed*u) as i32,visible:true};live+=1;}
                }
                if self.stop && live==0 {self.alive=false;}
            }
            _ => unreachable!(),
        }
        if mode!=4 && self.stop {self.alive=false;}
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn vertices(&mut self,time:f32,_camera:Vec3,right:Vec3,up:Vec3,_gc:&mut R250,_ds:&mut R250,crt:&mut CrtRand)->Result<Option<Vec<Vec<Vertex>>>> {
        if !self.alive || (self.duration>=0.0 && time>self.duration) {self.alive=false;return Ok(None);}
        self.simulate(time,crt);if !self.alive {return Ok(None);}
        let material=materials::MATERIALS[self.template.words[9] as usize];let mut out=Vec::with_capacity(128);
        for s in &self.sprites {
            let (sin,cos)=(-s.angle).sin_cos();let x=(right*cos+up*sin)*(s.size*0.5);let y=(-right*sin+up*cos)*(s.size*0.5);let pos=s.position+right*s.radius;
            let u=s.frame.rem_euclid(material.1 as i32) as f32/material.1 as f32;let v=(s.frame/material.1 as i32) as f32/material.2 as f32;let du=1.0/material.1 as f32;let dv=1.0/material.2 as f32;
            quad(&mut out,[pos-x-y,pos+x-y,pos-x+y,pos+x+y],[[u,v+dv],[u+du,v+dv],[u,v],[u+du,v]],render_color(if s.visible {s.color}else{[0.0;4]}));
        }
        Ok(Some(vec![out]))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn template(mode:u32)->Template {let mut words=vec![0;32];words[9]=8;words[10]=mode;words[26]=10f32.to_bits();words[28]=1f32.to_bits();words[29]=15f32.to_bits();words[30]=300;Template {kind:2005,words}}
    #[test] fn native_suns_parameters_and_graceful_drain() -> Result<()> {
        let mut gc=R250::new(1);let mut ds=R250::new(2);let mut crt=CrtRand::new(3);
        let mut t=template(4);t.words[18]=1f32.to_bits();t.words[19]=1f32.to_bits();t.words[22]=0; t.words[23]=0;
        assert_eq!(authored_color(&t,0.5),[0.5,0.0,0.0,0.5]);
        let mut s=Suns::new(&t,Mat4::IDENTITY,Mat4::from_translation(Vec3::X),0,&mut gc,&mut ds,&mut crt)?;
        s.simulate(0.01,&mut crt);assert_eq!(s.deadlines.iter().filter(|&&d|d>0.01).count(),3);
        s.terminate_gracefully();s.simulate(0.1,&mut crt);assert!(s.alive);s.simulate(0.4,&mut crt);assert!(!s.alive);
        t.words[10]=5;assert!(Suns::new(&t,Mat4::IDENTITY,Mat4::IDENTITY,0,&mut gc,&mut ds,&mut crt).is_err());Ok(())
    }
    #[test] fn native_suns_orbit_capacity_and_duration_word() -> Result<()> {
        let mut gc=R250::new(1);let mut ds=R250::new(2);let mut crt=CrtRand::new(3);
        for mode in 0..4 {let mut s=Suns::new(&template(mode),Mat4::IDENTITY,Mat4::IDENTITY,0,&mut gc,&mut ds,&mut crt)?;s.simulate(5.0,&mut crt);assert_eq!(s.models()[0].2,128);assert!(s.sprites.iter().any(|p|p.visible));assert_eq!(s.duration,10.0);s.terminate_gracefully();s.simulate(5.1,&mut crt);assert!(!s.alive);}Ok(())
    }
}
