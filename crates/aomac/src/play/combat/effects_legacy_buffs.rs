//! GC Font: ctor100df42d, loader100decc3, init100dedc8, SetText100dee29,
//! process100deb04, graceful100deba5. Metrics DAT102c4608, mapping100dede6.
use super::{quad, sprites, EffectConfig, Template};
use anyhow::{ensure, Result};
use ao_scene::{Blend, Vertex};
use glam::{Mat4, Vec3};

// Exact printable ASCII atlas (x,y,width), GC102c4608..102c4784.
const GLYPHS: [[u8;3];95] = [
[0,0,0],[0,0,2],[2,0,5],[7,0,6],[13,0,7],[20,0,8],[28,0,9],[37,0,3],
[40,0,4],[44,0,3],[47,0,8],[55,0,7],[62,0,3],[65,0,4],[69,0,3],[72,0,7],
[79,0,9],[88,0,3],[91,0,9],[100,0,8],[108,0,8],[116,0,8],[0,12,9],[9,12,8],
[17,12,9],[26,12,8],[34,12,3],[37,12,3],[40,12,5],[45,12,5],[50,12,5],[55,12,8],
[0,0,0],[63,12,11],[74,12,9],[83,12,9],[92,12,9],[101,12,9],[110,12,8],[118,12,10],
[0,24,10],[10,24,3],[13,24,6],[19,24,9],[28,24,8],[36,24,11],[47,24,10],[57,24,10],
[67,24,9],[76,24,11],[87,24,9],[96,24,9],[105,24,9],[114,24,10],[0,36,9],[9,36,14],
[23,36,8],[31,36,9],[40,36,8],[0,0,0],[48,36,7],[0,0,0],[55,36,7],[62,36,6],
[68,36,5],[73,36,7],[80,36,7],[87,36,7],[94,36,7],[101,36,7],[108,36,6],[114,36,8],
[122,36,6],[0,48,3],[3,48,7],[10,48,7],[17,48,3],[20,48,10],[30,48,7],[37,48,7],
[44,48,7],[51,48,7],[58,48,5],[63,48,7],[70,48,6],[76,48,7],[83,48,7],[90,48,10],
[100,48,7],[107,48,7],[114,48,7],[121,48,4],[0,0,0],[0,0,0],[0,0,0],
];
fn glyph(byte:u8)->[u8;3] {if (32..127).contains(&byte) {GLYPHS[(byte-32) as usize]}else{[0;3]}}
struct Letter {x:f32,width:f32,uv:[[f32;2];4]}
pub(super) struct Font {template:Template,source:Mat4,duration:f32,scale:f32,color:[f32;4],letters:Vec<Letter>,stopped:bool}
impl Font {
    pub(super) fn new(t:&Template,source:Mat4,c:EffectConfig)->Result<Self> {
        ensure!(t.kind==2014,"not a native Font template");t.word(31)?;
        for i in (1..=6).chain(11..=29) {t.float(i)?;}
        let source=sprites::connector(t,source)?;
        let mut effect=Self {template:t.clone(),source,duration:c.duration.unwrap_or(t.float(26)?),scale:c.scale.unwrap_or(t.float(28)?),color:super::buff200x::authored_color(t,0.0),letters:Vec::new(),stopped:false};
        if let Some(color)=c.start_color {effect.set_color(color);}
        Ok(effect)
    }
    // Native SetText is byte-oriented ANSI, with a terminating NUL. Space advances
    // five pixels but emits no sprite; zero-width glyphs still advance one pixel.
    pub(super) fn set_text(&mut self,text:&[u8])->Result<()> {
        let text=&text[..text.iter().position(|&b|b==0).unwrap_or(text.len())];
        ensure!(text.len()<=65536,"native Font text exceeds bounded input");
        let advance=|b:u8|if b==b' ' {5}else{u32::from(glyph(b)[2])+1};
        let mut x= -0.5*text.iter().map(|&b|advance(b)).sum::<u32>() as f32*self.scale;
        self.letters.clear();self.letters.reserve(text.len());
        for &b in text {let [u,v,w]=glyph(b);if b!=b' ' {let width=f32::from(w)*self.scale;
            let u=f32::from(u)/128.0;let v=f32::from(v)/64.0;let du=f32::from(w)/128.0;
            self.letters.push(Letter {x:x+(width+self.scale)*0.5,width,uv:[[u,v+12.0/64.0],[u+du,v+12.0/64.0],[u,v],[u+du,v]]});}
            x+=advance(b) as f32*self.scale;
        }Ok(())
    }
    pub(super) fn update_source(&mut self,source:Mat4)->Result<()> {self.source=sprites::connector(&self.template,source)?;Ok(())}
    pub(super) fn set_color(&mut self,color:[f32;4]) {self.color=color;}
    pub(super) fn terminate_gracefully(&mut self) {self.stopped=true;}
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {vec![(Some(self.template.word(9).unwrap_or(0) as usize),(0..self.letters.len() as u32).flat_map(|i|[i*4,i*4+2,i*4+3,i*4,i*4+3,i*4+1]).collect(),self.letters.len()*4)]}
    pub(super) fn blends(&self)->Vec<Blend> {vec![Blend::AlphaBlend]}
    pub(super) fn vertices(&mut self,time:f32,right:Vec3,up:Vec3)->Result<Option<Vec<Vec<Vertex>>>> {
        if self.stopped || (self.duration>=0.0 && time>self.duration) {return Ok(None);}
        let centre=self.source.w_axis.truncate()+Vec3::Y*(self.template.float(29)?*time)+up*1.5;
        let color=super::buff200x::render_color(self.color);let mut vertices=Vec::with_capacity(self.letters.len()*4);
        for letter in &self.letters {let pos=centre+right*letter.x;let x=right*(letter.width*0.5);let y=up*(self.scale*6.0);
            quad(&mut vertices,[pos-x-y,pos+x-y,pos-x+y,pos+x+y],letter.uv,color);}
        Ok(Some(vec![vertices]))
    }
}

#[derive(Clone,Copy)]
struct Edge {a:glam::Vec2,b:glam::Vec2,tangent:Vec3,solid:bool}
fn boundary(bytes:&[u8])->Result<Vec<Edge>> {
    let mut at=0;
    let mut word=||->Result<u32> {let end=at+4;ensure!(end<=bytes.len(),"truncated Fence boundary");let v=u32::from_le_bytes(bytes[at..end].try_into()?);at=end;Ok(v)};
    let count=word()? as usize;ensure!(count<=bytes.len()/8,"invalid Fence polygon count");
    let mut edges=Vec::new();
    for _ in 0..count {word()?;let n=word()? as usize;ensure!(n<=bytes.len()/16,"invalid Fence point count");
        let mut points=Vec::with_capacity(n);
        for _ in 0..n {let flags=word()?;let x=f32::from_bits(word()?);let y=f32::from_bits(word()?);let z=f32::from_bits(word()?);ensure!(x.is_finite()&&y.is_finite()&&z.is_finite(),"nonfinite Fence point");points.push((flags,glam::Vec2::new(x,-z)));}
        for i in 0..n {let (flags,b)=points[(i+1)%n];if flags&0x8000ffff==0 {continue;}let a=points[i].1;let d=b-a;let tangent=Vec3::new(d.x,0.0,d.y).normalize_or_zero();edges.push(Edge {a,b,tangent,solid:flags as u16!=0});}
    }Ok(edges)
}
#[derive(Clone,Copy)]
struct Spark {end:f32,position:Vec3,basis:glam::Mat3,flip:bool,color:[f32;4]}
impl Default for Spark {fn default()->Self {Self {end:-100.0,position:Vec3::ZERO,basis:glam::Mat3::IDENTITY,flip:false,color:[0.0;4]}}}
pub(super) struct Fence {template:Template,source:Mat4,edges:Vec<Edge>,sparks:[Spark;64],nearest:usize,candidate:usize,emission:f32,stopped:bool}
impl Fence {
    pub(super) fn new(t:&Template,source:Mat4,store:&ao_rdb::RecordStore,pfid:u32)->Result<Self> {
        ensure!(t.kind==2012,"not a native Fence template");for i in 10..=20 {t.float(i)?;}ensure!(t.float(18)?>0.0,"invalid Fence spark lifetime");
        let edges=match store.get(0xf4255,pfid)? {Some(bytes)=>boundary(&bytes)?,None=>Vec::new()};
        Ok(Self {template:t.clone(),source:sprites::connector(t,source)?,edges,sparks:[Spark::default();64],nearest:0,candidate:0,emission:0.0,stopped:false})
    }
    pub(super) fn update_source(&mut self,m:Mat4)->Result<()> {self.source=sprites::connector(&self.template,m)?;Ok(())}
    pub(super) fn terminate_gracefully(&mut self) {self.stopped=true;}
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {vec![(Some(self.template.word(9).unwrap_or(0) as usize),(0..64).flat_map(|i|[i*4,i*4+1,i*4+2,i*4+1,i*4+2,i*4+3]).collect(),256)]}
    pub(super) fn blends(&self)->Vec<Blend> {vec![Blend::Additive]}
    pub(super) fn vertices(&mut self,time:f32,crt:&mut ao_formats::character::CrtRand)->Result<Option<Vec<Vec<Vertex>>>> {
        let life=self.template.float(18)?;let width=self.template.float(20)?;let centre=self.source.w_axis.truncate();
        let closest=|edge:Edge| {let p=glam::Vec2::new(centre.x,centre.z);let d=edge.b-edge.a;let u=if d.length_squared()==0.0 {0.0}else{((p-edge.a).dot(d)/d.length_squared()).clamp(0.0,1.0)};let q=edge.a+d*u;((q-p).length_squared(),q)};
        if self.edges.is_empty() {self.stopped=true;}
        let mut point=glam::Vec2::ZERO;
        if !self.edges.is_empty() {if self.candidate==self.nearest {self.candidate+=1;}self.candidate%=self.edges.len();
            let (mut distance,q)=closest(self.edges[self.nearest]);point=q;let (other,q)=closest(self.edges[self.candidate]);
            if other<distance {self.nearest=self.candidate;distance=other;point=q;}self.candidate+=1;
            self.emission=(self.emission+self.template.float(19)?/distance).min(3.0);
        }
        let mut budget=self.emission as i32;self.emission-=budget as f32;
        let material=super::materials::MATERIALS.get(self.template.word(9)? as usize).ok_or_else(||anyhow::anyhow!("unknown Fence material"))?;
        let mut out=vec![Vertex {color:[0.0;4],..Default::default()};256];
        for (i,s) in self.sparks.iter_mut().enumerate() {
            if s.end<=time {if budget<=0||self.stopped {continue;}let edge=self.edges[self.nearest];
                let p=super::buff200x::circle(crt);let normal=Vec3::new(-edge.tangent.z,0.0,edge.tangent.x);
                s.position=Vec3::new(point.x,centre.y+1.0,point.y)+(normal*p.x+Vec3::Y*p.z)*2.0;
                let angle=(f64::from(crt.rand())/9990.2001953125) as f32;
                let tangent=glam::Quat::from_axis_angle(normal,angle)*Vec3::Y;
                s.basis=glam::Mat3::from_cols(tangent,normal,tangent.cross(normal));
                s.flip=crt.rand()&1==0;s.color=[11,12,13,10].map(|j|f32::from_bits(self.template.word(j+if edge.solid {4}else{0}).unwrap_or(0)));
                s.end=time+life;budget-=1;continue;
            }
            let phase=(life-(s.end-time))/life;let frame=(phase*16.0) as u32;
            let u=(frame%material.1) as f32/material.1 as f32;let v=(frame/material.1) as f32/material.2 as f32;
            let du=1.0/material.1 as f32;let dv=1.0/material.2 as f32;
            let a=s.basis.x_axis*width*if s.flip {-0.5}else{0.5};let b=s.basis.z_axis*width*0.5;
            let color=super::buff200x::render_color(s.color);
            for (j,(p,uv)) in [(s.position-a-b,[u,v+dv]),(s.position+a-b,[u+du,v+dv]),(s.position-a+b,[u,v]),(s.position+a+b,[u+du,v])].into_iter().enumerate() {out[i*4+j]=Vertex {pos:p.to_array(),uv,color,..Default::default()};}
        }
        if self.stopped&&self.sparks.iter().all(|s|s.end<=time) {Ok(None)}else{Ok(Some(vec![out]))}
    }
}

/// GC Notum2015: loader100eaeb9, init100eb20e, process100eaee9.
/// DS Cone18-vertex triangle strip1000bd0a/1000c0ec, priority6, SRCALPHA/ONE.
pub(super) struct Notum {mode:u32,source:Mat4,position:Vec3,phase:f32,offset:i32,previous:i32,sounds:Vec<(&'static str,Vec3)>}
impl Notum {
    pub(super) fn new(t:&Template,source:Mat4,creation:super::Creation)->Result<Self> {
        ensure!(t.kind==2015,"not a native Notum template");
        let mode=t.words.first().copied().unwrap_or(0);let (source,position)=match creation {
            super::Creation::Vector=>(Mat4::IDENTITY,source.w_axis.truncate()),
            super::Creation::RConnector=>(source,source.w_axis.truncate()),
            super::Creation::Dynel=>(Mat4::IDENTITY,Vec3::ZERO),
            _=>return Err(anyhow::anyhow!("Notum native constructor overload missing")),
        };
        let mut sounds=Vec::new();if mode==1 {sounds.push(("SM_Sandy_Ingame_Notun_Cannon_Idle",position));}
        Ok(Self {mode,source,position,phase:0.0,offset:(f64::from(position.x)*0.25) as i32,previous:0,sounds})
    }
    // Native graceful termination is a no-op; explicit Delete destroys the cone.
    pub(super) fn terminate_gracefully(&mut self) {}
    pub(super) fn take_sounds(&mut self)->Vec<(&'static str,Vec3)> {std::mem::take(&mut self.sounds)}
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {
        if self.mode!=0 {return Vec::new();}
        vec![(Some(13),(0..16u32).flat_map(|i|if i&1==0 {[i,i+1,i+2]}else{[i+1,i,i+2]}).collect(),18)]
    }
    pub(super) fn blends(&self)->Vec<Blend> {if self.mode==0 {vec![Blend::Additive]}else{Vec::new()}}
    pub(super) fn vertices(&mut self,dt:f32,seconds:i32,viewer:Vec3)->Result<Option<Vec<Vec<Vertex>>>> {
        self.phase+=dt*2.0;let tick=self.phase>1.0;if tick {self.phase-=1.0;}
        if self.mode!=0 {if self.mode==1&&tick {self.sounds.push(("SM_Sandy_Ingame_Notun_Cannon_Idle",self.position));}return Ok(Some(Vec::new()));}
        let second=(seconds+self.offset)%270;
        if second<self.previous {self.sounds.push((if viewer.distance(self.position)<300.0 {"SM_Sandy_Ingame_Notun_Cannon_Close"}else{"SM_Sandy_Ingame_Notun_Cannon_Medi"},self.position));self.sounds.push(("SM_Sandy_Ingame_Notun_Cannon_Far",viewer));}
        self.previous=second;
        let (height,radius)=match second {0=>(self.phase*200.0,self.phase*2.0+1.0),1=>(200.0,3.0),2=>((1.0-self.phase)*200.0,(1.0-self.phase)*2.0+1.0),_=>return Ok(Some(vec![vec![Vertex {color:[0.0;4],..Default::default()};18]]))};
        let rotation=glam::Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
        let mut out=Vec::with_capacity(18);
        for i in 0..=8 {let angle=i as f32*std::f32::consts::TAU/8.0;let (s,c)=angle.sin_cos();
            for (r,y,alpha,v) in [(1.0,0.0,1.0,0.0),(radius,height,34.0/255.0,1.0)] {
                let native=Vec3::new(c*r,y,s*r);let reflected=Vec3::new(native.x,native.y,-native.z);
                let position=self.position+self.source.transform_vector3(rotation*reflected);
                out.push(Vertex {pos:position.to_array(),color:[1.0,1.0,1.0,alpha],uv:[i as f32/8.0,v],..Default::default()});
            }
        }Ok(Some(vec![out]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn native_font_metrics_spacing_and_lifecycle()->Result<()> {
        let t=Template {kind:2014,words:vec![7,0,0,0,0,0,0,1000,3212836864,40,2,0,1056964608,1082130432,1077936128,1065353216,1077936128,1077936128,1065353216,1065353216,1065353216,1065353216,1065353216,1065353216,1065353216,1065353216,1067869798,0,1008981770,1053609165,0,0]};
        let mut f=Font::new(&t,Mat4::IDENTITY,EffectConfig::default())?;
        assert!(f.letters.is_empty());f.set_text(b"1 2\0ignored")?;assert_eq!(f.letters.len(),2);
        assert_eq!(glyph(b'1'),[88,0,3]);assert_eq!(glyph(b'2'),[91,0,9]);
        assert!((f.letters[1].x-f.letters[0].x-0.12).abs()<1e-6);
        assert!(f.vertices(1.0,Vec3::X,Vec3::Y)?.is_some());f.terminate_gracefully();assert!(f.vertices(1.0,Vec3::X,Vec3::Y)?.is_none());Ok(())
    }
    #[test] fn authored_notum_empty_payload_and_position_clock()->Result<()> {
        let t=Template {kind:2015,words:vec![]};
        let mut n=Notum::new(&t,Mat4::from_translation(Vec3::X*8.0),super::super::Creation::Vector)?;
        assert_eq!(n.offset,2);assert_eq!(n.models()[0].2,18);
        assert_eq!(n.vertices(0.1,268,Vec3::ZERO)?.unwrap()[0].len(),18);
        assert!((n.phase-0.2).abs()<1e-6);n.terminate_gracefully();assert_eq!(n.models()[0].2,18);Ok(())
    }
    #[test] fn native_boundary_destination_flags_and_truncation()->Result<()> {
        let mut bytes=Vec::new();
        for w in [1u32,0,2,1,0,0,0,0x80000000,10f32.to_bits(),0,0] {bytes.extend(w.to_le_bytes());}
        let edges=boundary(&bytes)?;assert_eq!(edges.len(),2);assert!(!edges[0].solid);assert!(edges[1].solid);
        assert!(boundary(&bytes[..bytes.len()-1]).is_err());Ok(())
    }
    #[test] fn authored_11600_fence_pool_and_graceful_drain()->Result<()> {
        let t=Template {kind:2012,words:vec![1,0,0,0,0,0,0,1000,3212836864,2,1065353216,1065353216,1056964608,1056964608,1065353216,1056964608,1056964608,1065353216,1061997773,1117782016,1061997773]};
        let edge=Edge {a:glam::Vec2::new(-10.0,1.0),b:glam::Vec2::new(10.0,1.0),tangent:Vec3::X,solid:true};
        let mut f=Fence {template:t,source:Mat4::IDENTITY,edges:vec![edge],sparks:[Spark::default();64],nearest:0,candidate:0,emission:0.0,stopped:false};
        let mut crt=ao_formats::character::CrtRand::new(7);
        f.vertices(0.0,&mut crt)?;assert_eq!(f.sparks.iter().filter(|s|s.end>0.0).count(),3);
        assert!(f.vertices(0.1,&mut crt)?.unwrap()[0].iter().any(|v|v.color[3]>0.0));
        f.terminate_gracefully();assert!(f.vertices(1.0,&mut crt)?.is_none());Ok(())
    }
    #[test]
    #[ignore = "installed authored legacy assets and offscreen Metal capture"]
    fn retail_legacy_buffs_all_authored_frames()->Result<()> {
        use super::super::{Templates,Renderer};
        use ao_scene::{Mesh,Submesh,Scene,TextureKey,ActorFrame};
        use anyhow::Context;
        let dir=ao_gui::client_dir();let templates=Templates::open(&dir)?;let renderer=Renderer::open(&dir)?;
        let out=std::path::PathBuf::from(std::env::var_os("AOMAC_EFFECT_FRAMES").context("AOMAC_EFFECT_FRAMES required")?);std::fs::create_dir_all(&out)?;
        let mut crt=ao_formats::character::CrtRand::new(1);
        for (&id,t) in templates.by_id.iter().filter(|(_,t)|matches!(t.kind,2012|2014|2015)) {
            let mut font=if t.kind==2014 {let mut f=Font::new(t,Mat4::IDENTITY,EffectConfig::default())?;f.set_text(b"123 Critical!")?;Some(f)}else{None};
            let mut fence=if t.kind==2012 {Some(Fence::new(t,Mat4::IDENTITY,&renderer.store,566)?)}else{None};
            let mut notum=if t.kind==2015 {Some(Notum::new(t,Mat4::IDENTITY,super::super::Creation::Vector)?)}else{None};
            let origin=if let Some(f)=&mut fence {let e=*f.edges.first().context("actual playfield566 Fence edges absent")?;let p=(e.a+e.b)*0.5;let origin=Vec3::new(p.x,0.0,p.y)+Vec3::new(-e.tangent.z,0.0,e.tangent.x);f.update_source(Mat4::from_translation(origin))?;origin}else{Vec3::ZERO};
            let eye=origin+Vec3::new(2.0,3.0,8.0);
            for frame in 1..=90 {
                let (vertices,models,blend)=if let Some(f)=&mut font {(f.vertices(frame as f32/60.0,Vec3::X,Vec3::Y)?,f.models(),Blend::AlphaBlend)}
                    else if let Some(f)=&mut fence {(f.vertices(frame as f32/60.0,&mut crt)?,f.models(),Blend::Additive)}
                    else {let n=notum.as_mut().unwrap();(n.vertices(1.0/60.0,frame/30,eye)?,n.models(),Blend::Additive)};
                if ![1,6,30,60,90].contains(&frame) {continue;}let Some(mut vertices)=vertices else{continue;};if models.is_empty(){continue;}
                let material=models[0].0.unwrap();let name=super::super::materials::MATERIALS[material].0;
                let key=TextureKey {rdb_type:1010004,id:renderer.names.id(1010004,name).context("legacy texture name absent")?};
                let texture=ao_formats::texture::load_texture(&renderer.store,key)?.context("legacy texture absent")?;
                let mut sub=Submesh::new(models[0].1.clone(),Some(key));sub.blend=blend;sub.two_sided=true;sub.emissive=[1.0;3];
                let mut scene=Scene::default();scene.textures.insert(key,texture);scene.meshes.push(Mesh {vertices:vertices.remove(0),submeshes:vec![sub]});
                scene.instances.extend((0..scene.meshes.len()).map(|mesh|ao_scene::Instance {mesh,transform:Mat4::IDENTITY.to_cols_array_2d()}));
                assert_eq!(scene.instances.len(),scene.meshes.len(),"every legacy buff fixture mesh must be drawable");
                ao_render::render_to_png_actors(&scene,&[],Vec::<ActorFrame>::new(),eye.to_array(),origin.to_array(),640,480,&out.join(format!("legacy_{}_{id}_{frame}.png",t.kind)),frame as f32/60.0)?;
            }
        }Ok(())
    }
}

#[cfg(test)]
#[test]
fn short_font_defaults_missing_fields_to_zero() {
    let mut effect=Font::new(&Template {kind:2014,words:Vec::new()},Mat4::IDENTITY,EffectConfig::default()).unwrap();
    effect.set_text(b"A").unwrap();
    assert_eq!(effect.models()[0].0,Some(0));
    assert_eq!(effect.scale,0.0);
    assert_eq!(effect.color,[0.0;4]);
}
