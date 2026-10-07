//! GC Shield3003: params100ed998, CAT callback100eda23, Process100ed79f,
//! graceful100ed747. DS posed-vertex callback1001cd09 ->1001c94f.
//! Input mesh is the live posed CAT surface, not a replacement primitive.
use super::{EffectConfig, Template};
use anyhow::{ensure, Result};
use ao_scene::{Blend, Vertex};
use glam::{Mat4, Vec3};

pub(super) struct Shield {
    template:Template, source:Mat4, identity:(u32,u32), vertices:Vec<Vertex>,indices:Vec<u32>,
    source_material:Option<usize>, elapsed:f32,duration:f32,started:bool,
    terminating:Option<(f32,f32)>,terminated:bool,
}
impl Shield {
    pub(super) fn new(t:&Template,source:Mat4,c:EffectConfig)->Result<Self> {
        ensure!(t.kind==3003,"not a shield template");
        for i in [8,11,12,13,14,15,16,17,19,20,21,22,23,24,25,27,28,30,31] {t.float(i)?;}
        ensure!(t.word(18)?<=2,"invalid native shield UV mode");
        ensure!(t.word(26)?<=2 && t.word(29)?<=3,"invalid native shield time mode");
        let identity=c.source_identity.ok_or_else(||anyhow::anyhow!("Shield requires a source dynel"))?;
        Ok(Self {template:t.clone(),source,identity,vertices:Vec::new(),indices:Vec::new(),source_material:None,elapsed:0.0,duration:c.duration.unwrap_or(t.float(8)?),started:false,terminating:None,terminated:false})
    }
    pub(super) fn identity(&self)->(u32,u32) {self.identity}
    pub(super) fn uses_source_material(&self)->bool {self.template.words.first().copied().unwrap_or(0)&0x10000!=0}
    pub(super) fn update_source(&mut self,source:Mat4) {self.source=source;}
    pub(super) fn update_mesh(&mut self,vertices:&[Vertex],indices:&[u32],material:Option<usize>)->Result<()> {
        ensure!(indices.iter().all(|&i|(i as usize)<vertices.len()),"invalid Shield source indices");
        self.vertices.clear();self.vertices.extend_from_slice(vertices);
        if self.indices!=indices {self.indices.clear();self.indices.extend_from_slice(indices);}
        self.source_material=material;Ok(())
    }
    pub(super) fn update_vertices(&mut self,vertices:&[Vertex])->Result<()> {
        ensure!(vertices.len()==self.vertices.len(),"Shield posed vertex count changed without mesh replacement");
        self.vertices.copy_from_slice(vertices);
        Ok(())
    }
    pub(super) fn terminate_gracefully(&mut self) {
        if self.terminating.is_none() {
            let low=f32::from_bits(self.template.words.get(27).copied().unwrap_or(0));let high=f32::from_bits(self.template.words.get(28).copied().unwrap_or(0));
            let phase=if low<high {self.elapsed%(high-low)+low}else{self.elapsed};
            self.terminating=Some((self.elapsed,phase));
        }
    }
    pub(super) fn frame(&mut self,dt:f32)->Result<bool> {
        ensure!(dt.is_finite() && dt>=0.0,"invalid Shield timestep");
        if self.started {self.elapsed+=dt;} else {self.started=true;}
        let fade=self.template.float(30)?;
        if let Some((start,_))=self.terminating {if self.elapsed-start>=fade {self.terminated=true;}}
        if self.duration>0.0 && self.elapsed>self.duration-fade {self.terminate_gracefully();}
        Ok(!self.terminated)
    }
    pub(super) fn alive(&self)->bool {!self.terminated}
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {
        let material=if self.uses_source_material() {self.source_material}else{Some(self.template.words.get(9).copied().unwrap_or(0) as usize)};
        vec![(material,self.indices.clone(),self.vertices.len())]
    }
    pub(super) fn blends(&self)->Vec<Blend> {vec![if self.template.words.first().copied().unwrap_or(0)&0x400!=0 {Blend::Additive}else{Blend::AlphaBlend}]}
    pub(super) fn vertices(&mut self)->Result<Option<Vec<Vec<Vertex>>>> {
        if !self.alive() {return Ok(None);}
        let t=&self.template;let flags=t.word(0)?;
        let mut time=self.elapsed;let mut alpha=1.0;
        if let Some((start,phase))=self.terminating {
            let since=self.elapsed-start;
            match t.word(29)? {1=>alpha=1.0-since/t.float(30)?,2=>time=phase-since,3=>time=t.float(31)?-since,_=>{}}
        }
        let low=t.float(27)?;let high=t.float(28)?;
        if time>low {
            let width=high-low;
            match t.word(26)? {1=>time=(time-low)%width+low,2=>time=(width-time%(2.0*width)).abs()+low,_=>{}}
        }
        let origin=Vec3::new(t.float(12)?,t.float(13)?,-t.float(14)?);
        let direction=Vec3::new(t.float(15)?,t.float(16)?,-t.float(17)?).normalize_or_zero();
        let packed=t.word(10)?;let mut color=super::buff300x::packed_color(packed);
        let mut out=Vec::with_capacity(self.vertices.len());
        for vertex in &self.vertices {
            let input=Vec3::from_array(vertex.pos);let normal=Vec3::from_array(vertex.normal);
            let p=input+normal*t.float(11)?;
            let uv=match t.word(18)? {
                0=>[(vertex.uv[0]+t.float(24)?*time)*t.float(19)?,(vertex.uv[1]+t.float(25)?*time)*t.float(20)?],
                // DS1001ca74 deliberately reuses the U scroll for cylindrical V.
                1=>[(-input.z).atan2(input.x)*t.float(19)?/std::f32::consts::TAU+t.float(24)?*time,input.y*t.float(20)?+t.float(24)?*time],
                _=>[input.x*t.float(19)?+t.float(24)?*time,input.y*t.float(20)?+t.float(25)?*time],
            };
            let distance=if flags&0x800==0 {0.0}else if flags&0x1000!=0 {(p-origin).dot(direction)}else{(p-origin).length()};
            // GC100eda23 -> DS1001ce93: phase fields +1e0/+1e4/+1e8 = w21/w22/w23.
            let phase=(t.float(21)?*time-distance*t.float(22)?).max(0.0).min(t.float(23)?);
            let mut a=((packed>>24) as f32*alpha*phase.sin().powi(2)).trunc() as u32&255;
            if flags&0x2000!=0 {
                // DS1001ccac: native per-normal modulation, applied after the envelope.
                let wave=(time*3.0+10.0+(normal.x+normal.y)*10.0*(-normal.z)).sin();
                a=(a as f32*wave*wave).trunc() as u32&255;
            }
            color[3]=a as f32/255.0;
            out.push(Vertex {pos:self.source.transform_point3(p).to_array(),normal:vertex.normal,uv,color});
        }
        Ok(Some(vec![out]))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn short_shield_zero_defaults_cover_frame_and_graceful_stop() -> Result<()> {
        let mut s=Shield::new(&Template {kind:3003,words:vec![]},Mat4::IDENTITY,EffectConfig {source_identity:Some((50000,1)),..Default::default()})?;
        assert!(!s.uses_source_material());assert_eq!(s.models()[0].0,Some(0));assert_eq!(s.blends(),vec![Blend::AlphaBlend]);
        assert!(s.frame(0.0)?);assert!(s.vertices()?.unwrap()[0].is_empty());
        s.terminate_gracefully();assert_eq!(s.terminating,Some((0.0,0.0)));assert!(!s.frame(0.0)?);Ok(())
    }
    #[test]
    fn shield_uses_posed_normals_and_native_fade() {
        let mut words=vec![0;32];words[0]=0x400;words[8]=(-1.0f32).to_bits();words[9]=13;words[10]=u32::MAX;
        for (i,v) in [(11,0.5),(19,1.0),(20,1.0),(21,1.0),(23,std::f32::consts::FRAC_PI_2),(30,1.0)] {words[i]=v.to_bits();}
        words[29]=1;
        let mut s=Shield::new(&Template {kind:3003,words},Mat4::IDENTITY,EffectConfig {source_identity:Some((50000,1)),..Default::default()}).unwrap();
        s.update_mesh(&[Vertex {pos:[1.0,2.0,3.0],normal:[0.0,1.0,0.0],uv:[0.0;2],color:[1.0;4]}],&[0,0,0],None).unwrap();
        s.frame(0.0).unwrap();s.frame(0.5).unwrap();
        assert_eq!(s.vertices().unwrap().unwrap()[0][0].pos,[1.0,2.5,3.0]);
        s.terminate_gracefully();assert!(s.frame(0.5).unwrap());assert!(!s.frame(0.5).unwrap());
    }
    #[test]
    fn shield_ctor_words_keep_phase_and_all_uv_modes_distinct() -> Result<()> {
        for (mode,expected_uv) in [(0,[7.0,20.25]),(1,[3.25,12.25]),(2,[7.25,16.25])] {
            let mut words=vec![0;32];words[0]=0x800|0x1000;words[8]=(-1.0f32).to_bits();words[10]=u32::MAX;words[18]=mode;
            for (i,v) in [(15,1.0f32),(19,2.0),(20,3.0),(21,1.5),(22,0.25),(23,0.75),(24,3.25),(25,4.25)] {words[i]=v.to_bits();}
            let mut s=Shield::new(&Template {kind:3003,words},Mat4::IDENTITY,EffectConfig {source_identity:Some((50000,1)),..Default::default()})?;
            s.update_mesh(&[Vertex {pos:[2.0,3.0,0.0],normal:[0.0;3],uv:[0.25,2.5],color:[1.0;4]}],&[],None)?;
            s.frame(0.0)?;
            assert_eq!(s.vertices()?.unwrap()[0][0].color[3],0.0,"negative phase clamps to zero");
            s.frame(1.0)?;
            let vertex=s.vertices()?.unwrap()[0][0];
            assert_eq!(vertex.uv,expected_uv,"native UV mode {mode}");
            // phase = min(1.5*time - signed_distance*0.25, 0.75).
            assert_eq!(vertex.color[3],(255.0*0.75f32.sin().powi(2)).trunc()/255.0);
            s.frame(0.5)?;
            // Exercise the uncapped signed-distance envelope independently of UV.
            s.template.words[23]=2.0f32.to_bits();
            let vertex=s.vertices()?.unwrap()[0][0];
            assert_eq!(vertex.color[3],(255.0*1.75f32.sin().powi(2)).trunc()/255.0);
            s.template.words[23]=4.0f32.to_bits();
            s.update_vertices(&[Vertex {pos:[-2.0,3.0,0.0],normal:[0.0;3],uv:[0.25,2.5],color:[1.0;4]}])?;
            let vertex=s.vertices()?.unwrap()[0][0];
            assert_eq!(vertex.color[3],(255.0*2.75f32.sin().powi(2)).trunc()/255.0,"negative signed distance increases phase");
        }
        Ok(())
    }
}
