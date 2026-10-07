//! GC3007 setup100da71f; DS10038ef9/100395d7/1003983e/10038f5d.
//! Allocation is trunc(length*rate*life*1.2000000476837158), not word12.
use super::super::{materials, quad, random_fraction, Template};
use anyhow::{ensure, Context, Result};
use ao_formats::weather::R250;
use ao_scene::{Blend, Vertex};
use glam::{Mat3, Vec3};

#[derive(Clone, Copy)]
struct Particle { phase:f32, position:Vec3, velocity:Vec3, width:f32, height:f32, growth:f32 }
pub(super) struct FallSteam {
    material:usize, color:u32, origin:Vec3, basis:Mat3, length:f32,
    bias:Vec3, offset:Vec3, start_size:f32, end_size:f32, rate:f32, life:f32, speed:f32,
    particles:Vec<Particle>, cursor:usize, emitted:u32, milliseconds:u32, previous:f32,
}
impl FallSteam {
    pub(super) fn new(t:&Template,start:Vec3,end:Vec3)->Result<Self> {
        ensure!(t.kind==3007,"not a FallSteam template");
        t.word(17)?;
        ensure!(start.is_finite() && end.is_finite(),"nonfinite FallSteam endpoints");
        let material=t.word(9)? as usize;
        materials::MATERIALS.get(material).context("unknown FallSteam material")?;
        let vector=|i|->Result<Vec3>{Ok(Vec3::new(t.float(i)?,t.float(i+1)?,-t.float(i+2)?))};
        let length=(end-start).length();let rate=length*t.float(14)?;let life=t.float(13)?;
        ensure!(rate>=0.0 && life>0.0,"invalid FallSteam emission/lifetime");
        let count=(rate as f64*life as f64*1.2000000476837158).trunc();
        ensure!(count<=u16::MAX as f64/4.0,"FallSteam capacity exceeds native indices");
        let forward=Vec3::new(end.x-start.x,0.0,end.z-start.z).normalize_or_zero();
        let forward=if forward==Vec3::ZERO {Vec3::Z}else{forward};
        Ok(Self {material,color:t.word(11)?,origin:(start+end)*0.5,basis:Mat3::from_cols(Vec3::Y.cross(forward),Vec3::Y,forward),length,
            bias:vector(4)?,offset:vector(1)?,start_size:t.float(16)?,end_size:t.float(17)?,rate,life,speed:t.float(15)?,
            particles:vec![Particle {phase:1.0,position:Vec3::ZERO,velocity:Vec3::ZERO,width:0.0,height:0.0,growth:0.0};count as usize],cursor:0,emitted:0,milliseconds:0,previous:0.0})
    }
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {
        let n=self.particles.len() as u32;
        vec![(Some(self.material),(0..n).flat_map(|i|[i*4,i*4+1,i*4+2,i*4,i*4+2,i*4+3]).collect(),n as usize*4)]
    }
    pub(super) fn blends(&self)->Vec<Blend> {vec![Blend::AlphaBlend]}
    fn advance(&mut self,dt:f32,ds:&mut R250) {
        if self.particles.is_empty() {return;}
        // DS accumulates integer milliseconds each frame; discarded fractions are not carried.
        self.milliseconds=self.milliseconds.wrapping_add((dt as f64*1000.0).trunc() as u32);
        let total=(self.milliseconds as f32/1000.0*self.rate).trunc() as u32;
        while self.emitted<total {
            let speed=((random_fraction(ds) as f64*0.2999999523162842+1.0)*self.speed as f64) as f32;
            let z=(random_fraction(ds)-0.5)*self.length;
            let noise=Vec3::new(random_fraction(ds)*2.0-1.0,random_fraction(ds)*2.0-1.0,-(random_fraction(ds)*2.0-1.0));
            let velocity=(noise+self.bias)*speed;
            let end=((random_fraction(ds) as f64*0.40000003576278687+0.800000011920929)*self.end_size as f64) as f32;
            let phase=(random_fraction(ds) as f64*0.10000000149011612) as f32;
            let width=((random_fraction(ds) as f64*0.40000003576278687+0.800000011920929)*self.start_size as f64) as f32;
            let height=((random_fraction(ds) as f64*0.40000003576278687+0.800000011920929)*self.start_size as f64) as f32;
            self.particles[self.cursor]=Particle {phase,position:self.offset+Vec3::new(0.0,0.0,-z),velocity,width,height,growth:(end-self.start_size)/self.life};
            self.cursor=(self.cursor+1)%self.particles.len();self.emitted+=1;
        }
        for p in &mut self.particles {if p.phase<1.0 {p.width+=p.growth*dt;p.height+=p.growth*dt;p.position+=p.velocity*dt;p.phase+=dt/self.life;}}
    }
    /// Reusable output hook: caller retains the Vec capacity between frames.
    pub(super) fn write_vertices(&mut self,time:f32,right:Vec3,up:Vec3,ds:&mut R250,out:&mut Vec<Vertex>)->Result<()> {
        ensure!(time.is_finite() && time>=self.previous,"invalid FallSteam frame time");
        let dt=time-self.previous;self.previous=time;self.advance(dt,ds);out.clear();
        let (_,columns,rows,first,last)=materials::MATERIALS[self.material];
        let du=1.0/columns as f32;let dv=1.0/rows as f32;
        let [a,r,g,b]=self.color.to_be_bytes();
        for p in &self.particles {
            let visible=p.phase<1.0;
            let fade=if p.phase<f32::from_bits(0x3ea8f5c3) {3.0*p.phase}else if p.phase>=f32::from_bits(0x3f2b851f) {3.0*(1.0-p.phase)}else{1.0};
            let alpha=if visible {(fade*a as f32).trunc()/255.0}else{0.0};
            let color=[(r as f32/255.0).powf(2.2),(g as f32/255.0).powf(2.2),(b as f32/255.0).powf(2.2),alpha];
            let frame=(first as f32+(last-first) as f32*p.phase).trunc() as u32;
            let u=(frame%columns) as f32*du;let v=(frame/columns) as f32*dv;
            let center=self.origin+self.basis*p.position;let x=right*p.width*0.5;let y=up*p.height*0.5;
            quad(out,[center-x+y,center-x-y,center+x-y,center+x+y],[[u,v],[u,v+dv],[u+du,v+dv],[u+du,v]],color);
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn class3007_native_capacity_rng_atlas_and_reused_geometry()->Result<()> {
        let mut t=Template {kind:3007,words:vec![0;18]};
        t.words[9]=28;t.words[11]=0xffffffff;t.words[12]=999;
        for (i,v) in [(13,1.0f32),(14,10.0),(15,1.0),(16,0.5),(17,1.0)] {t.words[i]=v.to_bits();}
        let mut effect=FallSteam::new(&t,Vec3::ZERO,Vec3::Z*2.0)?;
        assert_eq!(effect.particles.len(),24);assert_eq!(effect.models()[0].1[..6],[0,1,2,0,2,3]);
        let mut ds=R250::new(123);let mut reference=R250::new(123);for _ in 0..18 {reference.next_u32();}
        let mut out=Vec::with_capacity(96);let pointer=out.as_ptr();
        effect.write_vertices(0.1,Vec3::X,Vec3::Y,&mut ds,&mut out)?;
        assert_eq!(effect.emitted,2);assert_eq!(ds.next_u32(),reference.next_u32());
        assert_eq!(pointer,out.as_ptr());assert_eq!(out.len(),96);
        assert!(out.iter().all(|v|v.pos.iter().chain(v.color.iter()).all(|x|x.is_finite())));
        assert!(out.iter().any(|v|v.color[3]>0.0));
        assert!(out[0].uv[1]>=0.5 && out[0].uv[1]<1.0);
        effect.write_vertices(0.2,Vec3::X,Vec3::Y,&mut ds,&mut out)?;assert_eq!(pointer,out.as_ptr());
        Ok(())
    }
}
