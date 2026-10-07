//! GC ShockWave ctor100ee7f0/100ee96b, loader100ede77, init100ee576,
//! process100edfd0, delete100ee45e; DS ring10016e84 and cone1000bd0a.
//! GC Deformer ctor100d7ed6, loader100d7ce7, process100d7752,
//! vertex callback100d7217, graceful100d71ea, delete100d7bf5.
use super::{Template, EffectConfig};
use anyhow::{ensure, Result};
use ao_scene::{ActorFrame, Blend, Scene, Vertex};
use glam::{Mat4, Vec3};

fn color(a:u32,b:u32,t:f32)->[f32;4] {
    let a=a.to_be_bytes();let b=b.to_be_bytes();
    let bytes=std::array::from_fn(|i| (a[i] as f32*(1.0-t)+b[i] as f32*t) as u8);
    super::buff300x::packed_color(u32::from_be_bytes(bytes))
}
fn strip(n:u32)->Vec<u32> {(0..n*2).flat_map(|i|if i&1==0 {[i,i+1,i+2]}else{[i+1,i,i+2]}).collect()}

pub(super) struct ShockWave {t:Template, position:Vec3, centers:Vec<Option<Vec3>>, cones:Vec3, elapsed:f32, started:bool, stopped:bool,track:bool}
impl ShockWave {
    pub(super) fn new(t:&Template,source:Mat4)->Result<Self> {
        ensure!(t.kind==3000,"not ShockWave");t.word(35)?;
        for i in [8,12,13,16,17,20,21,22,23,27,28,29,30,31,32,33] {t.float(i)?;}
        ensure!((1..=32766).contains(&t.word(10)?) && t.word(11)?<=4096 && t.word(24)?<=4096,"invalid ShockWave counts");
        ensure!(t.float(8)?>0.0 && t.float(23)?>0.0,"invalid ShockWave timing");
        Ok(Self {t:t.clone(),position:source.w_axis.truncate(),centers:vec![None;t.word(11)? as usize],cones:source.w_axis.truncate(),elapsed:0.0,started:false,stopped:false,track:false})
    }
    // Native matrix/position setters reject updates. Dynel tracking uses flag1.
    pub(super) fn update_source(&mut self,m:Mat4) {if self.track && self.t.word(0).unwrap_or(0)&1!=0 {self.position=m.w_axis.truncate();}}
    pub(super) fn configure(&mut self,c:EffectConfig) {self.track=c.track_source&&c.source_identity.is_some();if let Some(d)=c.duration {self.t.words.resize(self.t.words.len().max(9),0);self.t.words[8]=d.to_bits();}}
    pub(super) fn terminate_gracefully(&mut self) {self.stopped=true;}
    pub(super) fn frame(&mut self,dt:f32)->Result<bool> {ensure!(dt.is_finite()&&dt>=0.0,"invalid ShockWave timestep");if self.started {self.elapsed+=dt;}else{self.started=true;}Ok(!self.stopped && self.elapsed<=self.t.float(8)?+self.t.float(23)?*(self.t.word(11).unwrap_or(0).saturating_sub(1)) as f32)}
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {
        (0..self.t.word(11).unwrap_or(0)).map(|_|(Some(self.t.word(9).unwrap_or(0) as usize),strip(self.t.word(10).unwrap_or(0)),(self.t.word(10).unwrap_or(0) as usize+1)*2))
        .chain((0..self.t.word(24).unwrap_or(0)).map(|_|(Some(self.t.word(25).unwrap_or(0) as usize),strip(self.t.word(26).unwrap_or(0)),(self.t.word(26).unwrap_or(0) as usize+1)*2))).collect()
    }
    pub(super) fn blends(&self)->Vec<Blend> {vec![if self.t.word(0).unwrap_or(0)&0x800!=0 {Blend::Additive}else{Blend::AlphaBlend};(self.t.word(11).unwrap_or(0)+self.t.word(24).unwrap_or(0)) as usize]}
    pub(super) fn vertices(&mut self,ground:&mut dyn FnMut(Vec3)->Option<(Vec3,Vec3)>)->Result<Option<Vec<Vec<Vertex>>>> {
        if self.stopped {return Ok(None)}
        let t=&self.t;let flags=t.word(0).unwrap_or(0);let spacing=t.float(23)?;
        let mut out=Vec::with_capacity((t.word(11).unwrap_or(0)+t.word(24).unwrap_or(0)) as usize);
        for ring in 0..t.word(11).unwrap_or(0) as usize {
            let phase=(self.elapsed-spacing*ring as f32)/t.float(8)?;
            let mut v=Vec::with_capacity((t.word(10).unwrap_or(0) as usize+1)*2);
            if phase>0.0 && phase<=1.0 {
                let center=*self.centers[ring].get_or_insert(self.position);
                for i in 0..=t.word(10).unwrap_or(0) {
                    let angle=std::f32::consts::TAU*i as f32/t.word(10).unwrap_or(0) as f32;
                    for end in 0..2 {
                        let mut radius=t.float(12+end)?*(1.0-phase)+t.float(16+end)?*phase;
                        if flags&0x2000!=0 {radius=radius.max(0.0);}
                        let mut p=center+Vec3::new(angle.cos()*radius,0.0,angle.sin()*radius);
                        p.y=ground(p).map_or(0.0,|(p,_)|p.y)+t.float(22)?;
                        let uv=if flags&0x1000!=0 {[i as f32*t.float(20)?/t.word(10).unwrap_or(0) as f32,end as f32*t.float(21)?]}else{[p.x*t.float(20)?,p.z*t.float(21)?]};
                        v.push(Vertex {pos:p.to_array(),normal:[0.0,1.0,0.0],uv,color:color(t.word(14+end).unwrap_or(0),t.word(18+end).unwrap_or(0),phase)});
                    }
                }
            } else {
                // DS10016e84 skips inactive rings; keep the combined actor skin fixed-size.
                v.resize((t.word(10).unwrap_or(0) as usize+1)*2,Vertex {pos:[0.0;3],normal:[0.0,1.0,0.0],uv:[0.0;2],color:[0.0;4]});
            }
            out.push(v);
        }
        let cycle=self.elapsed/spacing;let phase=cycle.fract();
        if phase<0.1 {self.cones=self.position;}
        let (bottom_alpha,top_alpha)=if phase<0.1 {(phase*10.0,phase*10.0)}else if phase<0.2 {(1.0,2.0-phase*10.0)}else {((1.0-(phase-0.2)/0.8).max(0.0),0.0)};
        for cone in 0..t.word(24).unwrap_or(0) {
            let mut v=Vec::with_capacity((t.word(26).unwrap_or(0) as usize+1)*2);
            if (cycle as u32)<t.word(11).unwrap_or(0) {
                for i in 0..=t.word(26).unwrap_or(0) {
                    let a=std::f32::consts::TAU*i as f32/t.word(26).unwrap_or(0) as f32;
                    for end in 0..2 {
                        let radius=if end==0 {t.float(30)?+cone as f32*t.float(32)?}else{t.float(31)?*t.float(33)?.powi(cone as i32)+cone as f32*t.float(32)?};
                        let p=self.cones+Vec3::new(a.cos()*radius,end as f32*t.float(29)?,a.sin()*radius);
                        let uv=if flags&0x4000!=0 {[end as f32*t.float(27)?,i as f32*t.float(28)?/t.word(26).unwrap_or(0) as f32]}else{[i as f32*t.float(27)?/t.word(26).unwrap_or(0) as f32,end as f32*t.float(28)?]};
                        let mut c=super::buff300x::packed_color(t.word(34+end).unwrap_or(0));c[3]*=if end==0 {bottom_alpha}else{top_alpha};
                        v.push(Vertex {pos:p.to_array(),normal:[0.0,1.0,0.0],uv,color:c});
                    }
                }
            } else {
                // DS1000bd0a skips expired cones without hiding the remaining rings.
                v.resize((t.word(26).unwrap_or(0) as usize+1)*2,Vertex {pos:[0.0;3],normal:[0.0,1.0,0.0],uv:[0.0;2],color:[0.0;4]});
            }
            out.push(v);
        }
        Ok(Some(out))
    }
}

struct Local {a:i32,b:i32,frequency:f32,phase:f32,radius:f32,strength:f32,center:Vec3}
pub(super) struct Deformer {t:Template,elapsed:f32,started:bool,stop:Option<(f32,f32)>,intensity:f32,actor:Option<u32>,locals:Vec<Local>}
impl Deformer {
    pub(super) fn new(t:&Template,c:EffectConfig)->Result<Self> {
        ensure!(t.kind==3001,"not Deformer");for i in [8,11,12,13] {t.float(i)?;}
        ensure!(matches!(t.word(10)?,0|1|4),"un-authored Deformer mode");
        let mut t=t.clone();if let Some(d)=c.duration {t.words.resize(t.words.len().max(9),0);t.words[8]=d.to_bits();}
        let mut locals=Vec::new();if t.word(10)?==0 {let n=t.word(14)?;ensure!(n<=4096,"Deformer attractor count");for i in 0..n as usize {let j=15+i*6;let radius=t.float(j+4)?;ensure!(radius>0.0,"Deformer radius");locals.push(Local {a:t.word(j)? as i32,b:t.word(j+1)? as i32,frequency:t.float(j+2)?,phase:t.float(j+3)?,radius,strength:t.float(j+5)?,center:Vec3::ZERO});}}
        if t.word(10)?==1 {t.float(14)?;t.float(15)?;}
        if t.word(10)?==4 {ensure!(t.float(12)?>0.0,"Deformer morph transition");}
        Ok(Self {t,elapsed:0.0,started:false,stop:None,intensity:0.0,actor:None,locals})
    }
    pub(super) fn mode(&self)->u32 {self.t.word(10).unwrap_or(0)}
    pub(super) fn prepare_source(&mut self,_scene:&Scene,actor:&ActorFrame)->Result<bool> {self.actor=Some(actor.id);Ok(false)}
    pub(super) fn terminate_gracefully(&mut self) {if self.stop.is_none() {self.stop=Some((self.elapsed,self.intensity));}}
    pub(super) fn frame(&mut self,dt:f32)->Result<bool> {
        ensure!(dt.is_finite()&&dt>=0.0,"invalid Deformer timestep");if self.started {self.elapsed+=dt;}else {self.started=true;}
        let duration=if self.mode()==4 {2.0*self.t.float(12)?+self.t.float(13)?}else{self.t.float(8)?};
        if duration>0.0 && self.elapsed>duration-self.t.float(13)? {self.terminate_gracefully();}
        self.intensity=if let Some((start,value))=self.stop {(1.0-(self.elapsed-start)/self.t.float(13)?)*value}else if self.elapsed<self.t.float(12)? {self.t.float(11)?*self.elapsed/self.t.float(12)?}else{self.intensity};
        Ok(self.stop.is_none()||self.intensity>0.0)
    }
    pub(super) fn update_anchors(&mut self,mut resolve:impl FnMut(i32)->Option<Mat4>,source:Mat4) {
        let inverse=source.inverse();for l in &mut self.locals {let a=resolve(l.a).map_or(Vec3::ZERO,|m|inverse.transform_point3(m.w_axis.truncate()));let b=resolve(l.b).map_or(Vec3::ZERO,|m|inverse.transform_point3(m.w_axis.truncate()));let f=0.5+0.5*(l.frequency*self.elapsed+l.phase).sin();l.center=a*f+b*(1.0-f);}
    }
    pub(super) fn deform(&self,vertices:&mut[Vertex],original:&[Vertex]) {
        for (v,rest) in vertices.iter_mut().zip(original) {
            let mut p=Vec3::from_array(v.pos);let n=Vec3::from_array(v.normal);
            if self.mode()==0 {for l in &self.locals {let distance=p.distance(l.center);let weight=(1.0-distance/l.radius).clamp(0.0,1.0);p+=n*(l.strength*weight*self.intensity);}}
            else if self.mode()==1 {let q=Vec3::from_array(rest.pos);let time=f32::from_bits(self.t.word(14).unwrap_or(0))*self.elapsed;let phase=((q.x+time*13.0)*13.0+(q.y-time*11.0)*17.0+q.z*time*19.0)*11.0+time;p+=n*(phase.sin().powi(2)*f32::from_bits(self.t.word(15).unwrap_or(0))*self.intensity);}
            else {let f=self.morph_fraction();let mut q=p;if q.y>1.5 {q.y-=1.5;q=q.normalize_or_zero()*0.2;q.y+=1.5;}else {let y=q.y;q.y=0.0;q=q.normalize_or_zero()*0.2;q.y=y;}p=p*(1.0-f)+q*f;}
            v.pos=p.to_array();
        }
    }
    pub(super) fn apply_actor(&self,actors:&mut[ActorFrame],original:&[Vertex]) {if self.mode()!=4 {if let Some(actor)=actors.iter_mut().find(|a|Some(a.id)==self.actor) {if let Some(v)=actor.skin.as_mut() {self.deform(v,original);}}}}
    pub(super) fn morph_fraction(&self)->f32 {let rise=f32::from_bits(self.t.word(12).unwrap_or(0));let hold=f32::from_bits(self.t.word(13).unwrap_or(0));if self.elapsed<rise {self.elapsed/rise}else if self.elapsed<=rise+hold {1.0}else{1.0-(self.elapsed-rise-hold)/rise}}
    pub(super) fn morph_alpha(&self)->f32 {self.morph_fraction()*0.7+0.15}
    pub(super) fn morph_child_should_stop(&self)->bool {self.elapsed>f32::from_bits(self.t.word(12).unwrap_or(0))+f32::from_bits(self.t.word(13).unwrap_or(0))}
    pub(super) fn transition_time(&self)->f32 {f32::from_bits(self.t.word(12).unwrap_or(0))}
}

pub(super) struct MorphVisual {
    rig:ao_formats::character::actor::ActorRig,
    clip:std::sync::Arc<ao_formats::character::CatAnim>,
}
impl MorphVisual {
    pub(super) fn load(store:&ao_rdb::RecordStore)->Result<Self> {
        let mut assets=ao_formats::character::actor::ActorAssets::new(store)?;
        let rig=ao_formats::character::actor::ActorRig::new(store,&assets,42886,None,&Default::default(),&Default::default(),&[])?;
        let clip=assets.anim(store,42892)?;
        Ok(Self {rig,clip})
    }
    pub(super) fn model(&self)->&Scene {self.rig.model()}
    pub(super) fn pose(&self,deformer:&Deformer)->(Vec<Vertex>,Vec<[[f32;4];4]>) {
        let (mut vertices,parts)=self.rig.pose(Some((&self.clip,deformer.elapsed*1000.0)));
        for v in &mut vertices {
            let p=Vec3::from_array(v.pos);let f=deformer.morph_fraction();let mut q=p;
            if q.y>1.5 {q.y-=1.5;q=q.normalize_or_zero()*0.2;q.y+=1.5;}
            else {let y=q.y;q.y=0.0;q=q.normalize_or_zero()*0.2;q.y=y;}
            v.pos=(p*(1.0-f)+q*f).to_array();
        }
        (vertices,parts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn short_native_records_zero_default_fields()->Result<()> {
        let mut d=Deformer::new(&Template {kind:3001,words:vec![]},EffectConfig {duration:Some(2.0),..Default::default()})?;
        assert_eq!(d.mode(),0);assert_eq!(d.t.float(8)?,2.0);assert_eq!(d.t.word(14)?,0);
        assert!(d.frame(0.0)?);
        let mut words=vec![0;24];words[8]=1.0f32.to_bits();words[10]=3;words[11]=1;words[23]=1.0f32.to_bits();
        let mut s=ShockWave::new(&Template {kind:3000,words},Mat4::IDENTITY)?;
        assert_eq!(s.models().len(),1);assert_eq!(s.t.word(24)?,0);
        s.frame(0.0)?;s.frame(0.5)?;
        let vertices=s.vertices(&mut |_|None)?.unwrap();
        assert_eq!(vertices.len(),1);assert_eq!(vertices[0][0].color,[0.0;4]);
        Ok(())
    }
    #[test]
    fn authored_30101_terrain_rings_and_cones() {
        let t=Template {kind:3000,words:vec![14337,0,0,0,0,0,0,0,1080033280,34,40,6,3221225472,0,4294938656,4294938656,1092616192,1094713344,16711935,16711935,1092616192,3212836864,1045891645,1053609165,5,13,10,1065353216,1065353216,1101004800,1050253722,1050253722,3175926989,1061158912,2164232192,16711935]};
        let mut s=ShockWave::new(&t,Mat4::IDENTITY).unwrap();assert_eq!(s.models().len(),11);
        s.frame(0.0).unwrap();s.frame(0.2).unwrap();
        let mut ground=|p:Vec3|Some((Vec3::new(p.x,7.0,p.z),Vec3::Y));
        let v=s.vertices(&mut ground).unwrap().unwrap();
        assert_eq!(v[0].len(),82);assert_eq!(v[6].len(),22);
        // GC100ede77 word22 -> +0x5c; GC100edfd0 adds packed 0x3e570a3d (0.21), not 0.2.
        assert!((v[0][0].pos[1]-7.21).abs()<1e-6);
        assert_eq!(v[1].len(),82);assert!(v[1].iter().all(|v|v.color==[0.0;4]));
        let models=s.models();
        // Birth, delayed rings, all-active window, expired cones, and final ring fade.
        for elapsed in [0.0,0.2,1.0,2.2,2.5,4.0,5.5,6.0] {
            s.elapsed=elapsed;
            let groups=s.vertices(&mut ground).unwrap().unwrap();
            assert_eq!(groups.len(),models.len());
            for (group,model) in groups.iter().zip(&models) {assert_eq!(group.len(),model.2,"elapsed {elapsed}");}
            assert_eq!(groups.iter().map(Vec::len).sum::<usize>(),models.iter().map(|m|m.2).sum::<usize>());
            for (ring,group) in groups[..6].iter().enumerate() {
                let phase=(elapsed-s.t.float(23).unwrap()*ring as f32)/s.t.float(8).unwrap();
                if phase<=0.0 || phase>1.0 {assert!(group.iter().all(|v|v.color==[0.0;4]));}
            }
            if elapsed>=s.t.float(23).unwrap()*6.0 {assert!(groups[6..].iter().flatten().all(|v|v.color==[0.0;4]));}
        }
        s.terminate_gracefully();assert!(!s.frame(0.0).unwrap());
    }
    #[test]
    fn authored_31101_localized_attractor_deformation() {
        let t=Template {kind:3001,words:vec![0,0,0,0,0,0,0,0,3212836864,0,0,1065353216,1073741824,1073741824,3,1017,2004,1077936128,0,1039516303,1031127695,1017,2004,1078774989,1051260355,1039516303,1031127695,1017,2004,1079613850,1059816735,1039516303,1031127695]};
        let mut d=Deformer::new(&t,EffectConfig::default()).unwrap();
        d.frame(0.0).unwrap();d.frame(1.0).unwrap();
        d.update_anchors(|_|Some(Mat4::IDENTITY),Mat4::IDENTITY);
        let rest=Vertex {pos:[0.0;3],normal:[0.0,1.0,0.0],uv:[0.0;2],color:[1.0;4]};
        // GC100d7217 applies each sphere to the already displaced destination:
        // radius0.12, strength0.06, intensity0.5 -> 0.03 + 0.0225 + 0.016875.
        let mut vertices=[rest];d.deform(&mut vertices,&[rest]);assert!((vertices[0].pos[1]-0.069375).abs()<1e-6);
        let mut far=rest;far.pos=[1.0,0.0,0.0];let mut vertices=[far];d.deform(&mut vertices,&[far]);assert_eq!(vertices[0].pos,far.pos);
    }
    #[test]
    fn authored_31201_envelope_and_callback() {
        let t=Template {kind:3001,words:vec![0,0,0,0,0,0,0,0,1066192077,0,1,1065353216,1036831949,1065353216,1036831949,1028443341]};
        let mut d=Deformer::new(&t,EffectConfig::default()).unwrap();
        assert!(d.frame(0.0).unwrap());assert!(d.frame(0.05).unwrap());
        let rest=Vertex {pos:[0.1,0.2,0.3],normal:[0.0,1.0,0.0],uv:[0.0;2],color:[1.0;4]};
        let mut vertices=[rest];d.deform(&mut vertices,&[rest]);assert!(vertices[0].pos[1]>rest.pos[1]);
        // GC100d71ea captures elapsed0.05 and intensity0.5; GC100d7752 kills at <=0.
        // f32 elapsed1.05 minus packed0.05 is 0.99999994, still inside word13's 1s fade.
        d.terminate_gracefully();assert!(d.frame(0.5).unwrap());assert!(d.frame(0.5).unwrap());
        assert!(!d.frame(0.01).unwrap());
    }
    #[test]
    fn authored_12276_morph_phase() {
        let t=Template {kind:3001,words:vec![0,0,0,0,0,0,0,0,3212836864,0,4,1065353216,1065353216,1073741824]};
        let mut d=Deformer::new(&t,EffectConfig::default()).unwrap();
        d.frame(0.0).unwrap();d.frame(0.5).unwrap();assert_eq!(d.morph_fraction(),0.5);assert!((d.morph_alpha()-0.5).abs()<1e-6);
        d.frame(2.0).unwrap();assert_eq!(d.morph_fraction(),1.0);d.frame(1.0).unwrap();assert_eq!(d.morph_fraction(),0.5);
    }
    #[test]
    #[ignore="installed authored native3000/3001 assets and offscreen GPU"]
    fn retail_native30001_authored_frames()->Result<()> {
        use super::super::{Binding,Renderer,MODEL_BASE};
        use anyhow::Context;
        let out=std::env::var_os("AOMAC_EFFECT_FRAMES").context("AOMAC_EFFECT_FRAMES required")?;
        let out=std::path::PathBuf::from(out);std::fs::create_dir_all(&out)?;
        let mut r=Renderer::open(&ao_gui::client_dir())?;
        let visual=MorphVisual::load(&r.store)?;
        let origin=Vec3::new(5000.0,10.0,5000.0);let source=Mat4::from_translation(origin);
        let eye=origin+Vec3::new(15.0,12.0,20.0);
        let mut host=ao_render::Host::headless();host.camera=ao_render::Camera::look_at(eye,origin);
        let raw=r.store.get(1_000_001,566)?.context("missing installed playfield566 resource")?;
        let resource=ao_formats::playfield::parse_resource(&raw)?;
        host.effect_playfield=Some((resource.tilemap,resource.flags));
        let mut environment_phase=0.0;
        let ids:Vec<_>=r.templates.by_id.iter().filter_map(|(&id,t)|matches!(t.kind,3000|3001).then_some(id)).collect();
        ensure!(ids.len()==16,"authored native3000/3001 census changed");
        for id in ids {
            r.clear();
            // Tracked native constructors resolve the source before the first frame.
            let (skin,parts)=visual.rig.pose(Some((&visual.clip,0.0)));
            let actor=ActorFrame {id:1,model:1,transform:source.to_cols_array_2d(),skin:Some(skin),parts,..Default::default()};
            r.prepare_source_mesh((50000,1),visual.model(),&actor);
            r.prepare_anchors((50000,1),|_,anchor|visual.rig.effect_anchor_composed(anchor,std::iter::empty()).map(|m|source*Mat4::from_cols_array_2d(&m)));
            let handle=r.spawn_configured(Binding {group:0,attractor:0,effect:id,note:0,color:0},source,origin,EffectConfig {creation:super::super::Creation::Dynel,source_identity:Some((50000,1)),track_source:true,..Default::default()})?;
            ensure!(handle!=0,"authored native class did not spawn: {id}");
            for frame in 1..=240 {
                host.actors.clear();
                let (skin,parts)=visual.rig.pose(Some((&visual.clip,frame as f32*1000.0/60.0)));
                let actor=ActorFrame {id:1,model:1,transform:source.to_cols_array_2d(),skin:Some(skin),parts,..Default::default()};
                r.prepare_source_mesh((50000,1),visual.model(),&actor);
                r.prepare_anchors((50000,1),|_,anchor|visual.rig.effect_anchor_composed(anchor,std::iter::empty()).map(|m|source*Mat4::from_cols_array_2d(&m)));
                host.actors.push(actor);
                let mut ground=|p:Vec3|Some((Vec3::new(p.x,10.0,p.z),Vec3::Y));
                host.effect_environment_center=Some(host.camera.pos.to_array());
                host.effect_environment_wind=Some(crate::play::flow::environment_channels(&mut environment_phase,1.0/60.0));
                r.frame(1.0/60.0,&mut host,Some(&mut ground));
                if [1,6,15,30,60,120,210,240].contains(&frame) {
                    let mut models:Vec<_>=r.models.iter().map(|(&id,m)|(MODEL_BASE|id as u32 as u64,m.scene.clone())).collect();
                    models.extend(r.buff_models.iter().map(|(&id,m)|(0xfac2_0000_0000_0000|u64::from(id),m.scene.clone())));
                    models.push((1,visual.model().clone()));
                    ao_render::render_to_png_actors(&Scene::default(),&models,host.actors.clone(),eye.to_array(),origin.to_array(),640,480,&out.join(format!("native30001_{id}_{frame}.png")),frame as f32/60.0)?;
                }
            }
        }
        Ok(())
    }
}
