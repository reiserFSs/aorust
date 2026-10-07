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
pub(super) struct Curve { knots:Vec<Knot> }
impl Curve {
    pub(super) fn parse(t:&Template,at:&mut usize,color:bool)->Result<Self> {
        let count=t.words.get(*at).copied().unwrap_or(0) as usize;*at+=1;
        // GC10106872/10106893 return zero beyond the template, including
        // authored71320's nine-knot offset curve with only five stored pairs.
        let end=at.checked_add(count.checked_mul(2).ok_or_else(||anyhow::anyhow!("native effect curve index overflow"))?).ok_or_else(||anyhow::anyhow!("native effect curve index overflow"))?;
        let stored=count.min(t.words.len().saturating_sub(*at).div_ceil(2));
        let mut knots=Vec::with_capacity(stored+usize::from(stored<count));
        for _ in 0..stored {
            let phase=if *at<t.words.len() {t.float(*at)?}else{0.0};
            let value=t.words.get(*at+1).copied().unwrap_or(0);
            if !color && *at+1<t.words.len() {t.float(*at+1)?;}
            knots.push((phase,value));*at+=2;
        }
        // Repeated implicit (0,0) knots have no interpolation interval.
        if stored<count {knots.push((0.0,0));}
        *at=end;
        Ok(Self {knots})
    }
    fn segment(&self,phase:f32)->Option<(Knot,Knot,f32)> {
        self.knots.windows(2).find(|w|w[0].0<=phase && phase<w[1].0).map(|w|(w[0],w[1],(phase-w[0].0)/(w[1].0-w[0].0)))
    }
    pub(super) fn scalar(&self,phase:f32)->f32 {
        if let Some((a,b,u))=self.segment(phase) {(1.0-u)*f32::from_bits(a.1)+u*f32::from_bits(b.1)}
        else {self.knots.last().map(|k|f32::from_bits(k.1)).unwrap_or(1.0)}
    }
    pub(super) fn color(&self,phase:f32)->[f32;4] {
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
    ring_position: f32,
}
impl ScatterEffect {
    pub(super) fn new(t:&Template, source:Mat4, target:Vec3, config:EffectConfig, random:&mut R250)->Result<Self> {
        ensure!(t.kind==3029,"not native Scatter");
        
        ensure!(t.word(4)?<=1 && t.word(5)?<=1,"invalid native Scatter placement/schedule mode");
        for i in [1,2,3,7,8,9] {t.float(i)?;}
        let count=t.word(6)? as usize;
        ensure!(count<=u16::MAX as usize,"Scatter capacity exceeds native index range");
        let duration=t.float(7)?;
        let slots=(0..count).map(|i|ScatterSlot {delay:if t.words.get(5).copied().unwrap_or(0) == 0 {super::random_fraction(random)*duration}else{duration*i as f32/count as f32},fired:false,child:0}).collect();
        // SetDuration, SetColor and SetNumRepetitions are base no-ops in this vtable.
        Ok(Self {template:t.clone(),source,target,config,slots,elapsed:0.0,cycle_start:0.0,terminating:false,ring_position:0.0})
    }
    pub(super) fn update_source(&mut self,source:Mat4) {self.source=source;}
    pub(super) fn graceful(&mut self,renderer:&mut Renderer)->Result<()> {
        if let Some(flags)=self.template.words.first_mut() {*flags&=!0x400;}
        for slot in &self.slots {if slot.child!=0 {renderer.terminate_gracefully(slot.child);}}
        self.terminating=true;
        Ok(())
    }
    pub(super) fn cancel(&mut self,renderer:&mut Renderer)->Result<()> {
        for slot in &mut self.slots {if slot.child!=0 {renderer.delete(slot.child);slot.child=0;}}
        Ok(())
    }
    pub(super) fn frame(&mut self,dt:f32,renderer:&mut Renderer,mut terrain:Option<&mut dyn FnMut(Vec3)->Option<(Vec3,Vec3)>>)->Result<bool> {
        self.elapsed+=dt;
        let t=&self.template;
        if self.terminating && t.words.first().copied().unwrap_or(0) & 0x4000!=0 {return Ok(false);}
        let mut alive=false;
        for slot in &mut self.slots {
            if slot.child!=0 && !renderer.is_active(slot.child) {slot.child=0;}
            if slot.child==0 && !slot.fired {
                if slot.delay>self.elapsed-self.cycle_start {alive=true;continue;}
                // Native samples three values even when the authored radius is zero.
                let direction=Vec3::new(super::random_fraction(&mut renderer.random)*2.0-1.0,super::random_fraction(&mut renderer.random)*2.0-1.0,-(super::random_fraction(&mut renderer.random)*2.0-1.0));
                // GC10110357: mode1 is jitter plus a progressing native-Z
                // coordinate, not a circular polar ring.
                let offset=if t.words.get(4).copied().unwrap_or(0) == 0 {direction*t.float(8)?} else {
                    let p=direction*t.float(9)?-Vec3::Z*self.ring_position;
                    self.ring_position+=t.float(8)?/t.word(6)? as f32;
                    p
                };
                let mut source=self.source;
                let mut config=self.config;
                config.duration=None;
                if t.words.first().copied().unwrap_or(0) & 0x2000==0 {
                    let mut position=self.source.w_axis.truncate()+offset;
                    if t.words.first().copied().unwrap_or(0) & 0x1000!=0 {
                        let locator=terrain.as_deref_mut().ok_or_else(||anyhow::anyhow!("Scatter requires terrain locator"))?;
                        let (ground,_)=locator(position).ok_or_else(||anyhow::anyhow!("Scatter terrain position is unavailable"))?;
                        position.y=ground.y;
                    }
                    position+=Vec3::new(t.float(1)?,t.float(2)?,-t.float(3)?);
                    source=Mat4::from_translation(position);
                    config.source_identity=None;
                    config.creation=super::Creation::Vector;
                }
                let id=t.word(10)? as i32;
                if id!=0 {slot.child=renderer.spawn_configured(Binding {group:0,attractor:0,effect:id,note:0,color:0},source,self.target,config)?;}
                slot.fired=true;
            }
            alive|=slot.child!=0;
        }
        if !alive && t.words.first().copied().unwrap_or(0) & 0x400!=0 {
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
    #[test]
    fn truncated_native_surface_templates_keep_zero_field_defaults() {
        let mut gc=R250::new(1);let mut ds=R250::new(1);let mut crt=ao_formats::character::CrtRand::new(1);
        for (kind,len) in [(3031,34),(3030,11),(3038,9),(3034,0),(3039,12)] {
            let mut words=vec![0;len];
            match kind {
                3031=>{for i in [25,26,33] {words[i]=1.0f32.to_bits();}},
                3030=>words[10]=2,
                3038=>words[8]=1.0f32.to_bits(),
                3039=>words[11]=1,
                _=>{}
            }
            for end in 0..=len {
                let t=Template {kind,words:words[..end].to_vec()};
                let mut accepted=false;
                match kind {
                    3031=>{if let Ok(mut e)=super::super::crystal::Crystal::new(&t,Mat4::IDENTITY,&mut gc) {
                        accepted=true;
                        e.models();e.blends();e.vertices(0.0,Vec3::Z,&mut gc,None).unwrap();
                    }},
                    3030=>{if let Ok(mut e)=super::super::groundgrid::GroundGrid::new(&t,Mat4::IDENTITY,EffectConfig::default(),&mut gc) {
                        accepted=true;
                        e.models();e.blends();e.vertices(0.0,&mut |p|Some((p,Vec3::Y))).unwrap();
                    }},
                    3038=>{if let Ok(mut e)=GridEffect::new(&t,Mat4::IDENTITY,Mat4::IDENTITY,0,&mut gc,&mut ds,&mut crt) {
                        accepted=true;
                        assert_eq!(e.models()[0].2,0);
                        e.vertices(0.0,Vec3::Z,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt).unwrap();
                    }},
                    3034=>{let mut e=ShieldEffect::new(&t,EffectConfig::default()).unwrap();
                        accepted=true;
                        assert_eq!(e.layer_count(),0);assert_eq!(e.material(),0);e.frame(0.0);
                        assert!(e.vertices(&[Vertex::default()],1.0).is_empty());
                    },
                    3039=>{if let Ok(mut e)=TrailEffect::new(&t,Mat4::IDENTITY,Mat4::IDENTITY,0,&mut gc,&mut ds,&mut crt) {
                        accepted=true;
                        e.models();e.vertices(0.0,Vec3::Z,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt).unwrap();
                    }},
                    _=>unreachable!()
                }
                if end==len {assert!(accepted,"native class {kind} must accept absent optional fields");}
            }
        }
    }
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
    #[test]
    fn authored_71320_mech_cylindrical_uv_and_71319_stop_child() {
        let words=vec![7168,0,0,0,0,0,0,0,1098907648,0,1,1,58,1077936128,1094713344,1077936128,3253731328,0,1101004800,3,0,3,0,9494015,1062836634,2861620735,1065353216,4287684095,9,0,0,1048576000,0,1056964608,1008981770,1061158912,1017370378,1065353216,1022739087,0];
        let effect=ShieldEffect::new(&Template {kind:3034,words},EffectConfig::default()).unwrap();
        assert_eq!(effect.mech_resource(),Some("EP03_preorder_mech_simple.abiff"));
        assert_eq!(effect.offset.knots.last(),Some(&(0.0,0)));
        assert_eq!(effect.offset.scalar(1.0),0.0);
        assert_eq!(effect.stop_child,0);
        let out=effect.vertices(&[Vertex {pos:[1.0,2.0,0.0],..Default::default()}],1.0);
        assert_eq!(out.len(),3);assert!((out[0].uv[0]-0.75).abs()<1e-6);assert_eq!(out[0].uv[1],24.0);
        let words=vec![1024,0,0,0,0,0,0,0,1101004800,0,0,1,58,1077936128,1077936128,1077936128,1088421888,0,1107296256,15,4294967295,4,0,9494015,1058642330,814800383,1060320051,814800383,1065353216,0,4,0,0,1050253722,1036831949,1063339950,1045220557,1065353216,0,71360];
        let mut effect=ShieldEffect::new(&Template {kind:3034,words},EffectConfig::default()).unwrap();
        assert_eq!(effect.stop_child,71360);
        assert_eq!(effect.take_stop_child(),0);effect.stop_surface();
        assert_eq!(effect.take_stop_child(),71360);assert_eq!(effect.take_stop_child(),0);
    }
    #[test]
    fn authored_72420_trail_uses_vehicle_direction_not_connector_axis() {
        let words=vec![7171,0,0,0,0,0,0,2006,3212836864,0,46,8,0,1056964608,1090519040,1077936128,4,0,4287692688,1055286886,4291624908,1057803469,4291624908,1065353216,4287692688,3,0,1034147594,1056964608,1039516303,1065353216,1034147594,3,0,1045220557,1056964608,1048576000,1065353216,1045220557];
        let mut gc=R250::new(7);let mut ds=R250::new(7);let mut crt=ao_formats::character::CrtRand::new(1);
        let mut trail=TrailEffect::new(&Template {kind:3039,words},Mat4::IDENTITY,Mat4::IDENTITY,0,&mut gc,&mut ds,&mut crt).unwrap();
        trail.set_vehicle_direction(Vec3::X);
        trail.update_source(Mat4::from_translation(Vec3::X)).unwrap();
        trail.vertices(0.1,Vec3::ZERO,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt).unwrap();
        assert!(trail.origin.color[3]>0.0);assert_eq!(trail.count,1);
        trail.update_source(Mat4::IDENTITY).unwrap();
        trail.vertices(0.2,Vec3::ZERO,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt).unwrap();
        assert_eq!(trail.origin.color,[0.0;4]);
        trail.graceful();assert!(trail.vertices(0.3,Vec3::ZERO,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt).unwrap().is_none());
    }
    #[test]
    #[ignore="installed authored texture resource holes"]
    fn retail_missing_surface_textures_retain_native_geometry()->Result<()> {
        let r=Renderer::open(&ao_gui::client_dir())?;
        assert!(r.names.id(1010004,"the_wave.png").is_none());
        assert!(r.store.get(1010004,8714)?.is_none());
        let missing_payload=super::super::materials::MATERIALS.iter().position(|m|r.names.id(1010004,m.0)==Some(8714)).expect("installed material referencing texture8714");
        for material in [98,missing_payload] {
            let mut words=vec![0;22];words[8]=3.0f32.to_bits();words[9]=material as u32;words[14]=1;words[15]=1;words[16]=1;
            let mut gc=R250::new(7);let mut ds=R250::new(7);let mut crt=ao_formats::character::CrtRand::new(1);
            let buff=super::super::buffs::Buff::new(&Template {kind:3038,words},Mat4::IDENTITY,Vec3::ZERO,0,EffectConfig::default(),&mut gc,&mut ds,&mut crt,None,None)?;
            let model=Renderer::build_buff_model(&r.store,&r.names,&buff)?;
            assert!(model.scene.textures.is_empty());
            assert_eq!(model.scene.meshes[0].vertices.len(),15);
            assert_eq!(model.scene.meshes[0].submeshes[0].indices.len(),36);
            assert!(model.scene.meshes[0].submeshes[0].texture.is_none());
        }
        Ok(())
    }
    #[test]
    #[ignore="installed authored assets and offscreen GPU regression"]
    fn retail_surface_authored_modes_frames()->Result<()> {
        use anyhow::Context;
        use super::super::MODEL_BASE;
        use ao_formats::character::{actor::{ActorAssets,ActorRig,PlayerLook},Breed,Gender,Skin,Equipment};
        let out=std::env::var_os("AOMAC_EFFECT_FRAMES").ok_or_else(||anyhow::anyhow!("AOMAC_EFFECT_FRAMES required"))?;
        let out=std::path::PathBuf::from(out);std::fs::create_dir_all(&out)?;
        let mut r=Renderer::open(&ao_gui::client_dir())?;
        let assets=ActorAssets::new(&r.store)?;
        let look=PlayerLook {breed:Breed::Solitus,gender:Gender::Male,skin:Skin::Caucasian,build:1,head:None,equipment:Equipment::default()};
        let rig=ActorRig::player(&r.store,&assets,&look,&[(1,15839)])?;
        // Account for actor-only frames using the installed CAT and native gates.
        let (posed,_)=rig.pose(None);
        assert!(posed.len()>1000,"installed CAT must exercise DS1001dc74 large-surface gate");
        for id in [96111,72274,72275,72276] {
            let effect=ShieldEffect::new(&r.templates.by_id[&id],EffectConfig::default())?;
            assert!(effect.vertices(&posed,1.0).is_empty(),"Shield2 {id} native CAT gate");
        }
        for (id,alpha) in [(43608,5.0/255.0),(43748,0.0)] {
            let mut effect=super::super::buff_shield::Shield::new(&r.templates.by_id[&id],Mat4::IDENTITY,EffectConfig {source_identity:Some((50000,1)),..Default::default()})?;
            effect.update_mesh(&posed,&[],None)?;effect.frame(0.0)?;effect.frame(1.0)?;
            let vertices=effect.vertices()?.context("installed Shield expired before sample")?;
            assert!(!vertices[0].is_empty());
            assert!(vertices[0].iter().all(|v|(v.color[3]-alpha).abs()<1e-6),"Shield {id} authored sin-squared alpha cap");
        }
        let origin=Vec3::new(5000.0,10.0,5000.0);let eye=origin+Vec3::new(4.0,3.0,8.0);
        let identity=(50000,1);
        // Material isolation for Meta25002/Highlight11502, not retail lighting.
        let highlight_world=ao_scene::Scene {environment:Some(ao_scene::Environment {sky_color:[0.0;3],fog_color:[0.0;3],fog_start:100.0,fog_end:200.0,ambient:[0.0;3],sun_color:[0.0;3],sun_dir:[0.0,1.0,0.0],sun_specular:0.0}),..Default::default()};
        let ids:Vec<_>=r.templates.by_id.iter().filter_map(|(&id,t)|matches!(t.kind,2004|2007|3003|3006|3007|3029|3032|3034|3036|3038|3039).then_some(id)).collect();
        ensure!(!ids.is_empty(),"missing authored surface effects");
        assert_eq!(ids.iter().filter(|id|r.templates.by_id[*id].kind==3036).count(),5,"installed Toggle frame coverage");
        let mut host=ao_render::Host::headless();host.camera=ao_render::Camera::look_at(eye,origin+Vec3::Y);
        for id in ids {
            r.clear();
            let t=&r.templates.by_id[&id];
            let toggle=t.kind==3036;
            host.effect_playfield=if toggle {
                // Exercise the authored include/exclude and mask gates without
                // changing the installed Toggle or its child template.
                let listed=&t.words[5..5+t.word(4)? as usize];
                let playfield=if t.words.first().copied().unwrap_or(0) & 0x800!=0 {
                    (0..=listed.len() as u32).find(|id|!listed.contains(id)).context("Toggle excludes every playfield")?
                } else {
                    *listed.first().context("Toggle has no permitted playfield")?
                };
                Some((playfield,t.word(3)?))
            } else {None};
            r.prepare_anchors(identity,|_,id|rig.effect_anchor(id,None).map(|m|Mat4::from_translation(origin)*Mat4::from_cols_array_2d(&m)));
            r.set_source_runtime(identity,1.0,1,None,0,true);
            let handle=r.spawn_configured(Binding {group:0,attractor:0,effect:id,note:0,color:0},Mat4::from_translation(origin),origin,EffectConfig {source_identity:Some(identity),creation:super::super::Creation::Dynel,..Default::default()})?;
            for frame in 1..=60 {
                let (skin,parts)=rig.pose(None);
                let actor=ao_scene::ActorFrame {id:1,model:1,transform:Mat4::from_translation(origin).to_cols_array_2d(),skin:Some(skin),parts,part_attractors:rig.part_attractors(),..Default::default()};
                r.prepare_source_mesh(identity,rig.model(),&actor);host.actors.clear();host.actors.push(actor);
                r.prepare_anchors(identity,|_,id|rig.effect_anchor(id,None).map(|m|Mat4::from_translation(origin)*Mat4::from_cols_array_2d(&m)));
                r.set_source_runtime(identity,if toggle && frame>30 {0.0}else{1.0},1,None,0,true);
                let mut terrain=|p:Vec3|Some((Vec3::new(p.x,10.0,p.z),Vec3::Y));
                r.frame(1.0/60.0,&mut host,Some(&mut terrain));
                if id==25002 && frame==30 {
                    let source=host.actors.iter().find(|actor|actor.id==1).context("missing Meta source actor")?;
                    let emissive=source.emissive.context("Meta25002 child11502 must apply root material")?;
                    assert!(emissive.iter().all(|channel|*channel>0.0),"mode1 midpoint must change root emissive");
                }
                // Include the second tick: authored 20ms Burst children can be
                // invisible at tick1 and already expired by the old tick15 sample.
                if [1,2,15,30,45,60].contains(&frame) {
                    let mut models=vec![(1,rig.model().clone())];
                    models.extend(r.models.iter().map(|(&id,m)|(MODEL_BASE|id as u32 as u64,m.scene.clone())));
                    models.extend(r.buff_models.iter().map(|(&id,m)|(0xfac2_0000_0000_0000|u64::from(id),m.scene.clone())));
                    let world=if id==25002 {&highlight_world}else{&ao_scene::Scene::default()};
                    ao_render::render_to_png_actors(world,&models,host.actors.clone(),eye.to_array(),(origin+Vec3::Y).to_array(),640,480,&out.join(format!("surface_{id}_{frame}.png")),frame as f32/60.0)?;
                }
            }
            r.delete(handle);assert!(!r.is_active(handle));
        }
        Ok(())
    }
}

/// GC10115a3b/10115d41; DS1002f86d/1002f69e/100302ad.
pub(super) struct GridEffect {template:Template,source:Mat4,position:Vec3,curves:[Curve;5],duration:f32,previous:f32,angle:f32}
impl GridEffect {
    // GC10115a3b loads words10/11/12, but GC10115964 and10115d41 never
    // forward or read them: installed mode11=4/scale12=1 are inert natively.
    pub(super) fn new(t:&Template,source:Mat4,_target:Mat4,_color:u32,gc:&mut R250,_ds:&mut R250,_crt:&mut ao_formats::character::CrtRand)->Result<Self> {
        ensure!(t.kind==3038,"not native VolGrid");
        for i in [1,2,3,4,5,6,8,12,13] {t.float(i)?;}
        ensure!(t.float(8)?>0.0,"invalid VolGrid duration");
        ensure!((14..17).map(|i|u64::from(t.words.get(i).copied().unwrap_or(0))).sum::<u64>()<=u16::MAX as u64/5,"VolGrid index overflow");
        super::materials::MATERIALS.get(t.words.get(9).copied().unwrap_or(0) as usize).ok_or_else(||anyhow::anyhow!("unknown VolGrid material"))?;
        let mut at=17;
        let curves=[Curve::parse(t,&mut at,false)?,Curve::parse(t,&mut at,false)?,Curve::parse(t,&mut at,false)?,Curve::parse(t,&mut at,true)?,Curve::parse(t,&mut at,true)?];
        let source=super::sprites::connector(t,source)?;
        let angle=if t.words.first().copied().unwrap_or(0) & 0x1000!=0 {super::random_fraction(gc)*std::f32::consts::TAU-std::f32::consts::PI}else{0.0};
        Ok(Self {template:t.clone(),source,position:source.w_axis.truncate(),curves,duration:t.float(8)?,previous:0.0,angle})
    }
    // Native SetDuration and TerminateGracefully are no-ops (100793e8/10115963).
    pub(super) fn configure(&mut self,_config:EffectConfig)->Result<()> {Ok(())}
    pub(super) fn update_source(&mut self,source:Mat4)->Result<()> {self.source=super::sprites::connector(&self.template,source)?;Ok(())}
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {
        let n=(14..17).map(|i|self.template.words.get(i).copied().unwrap_or(0)).sum::<u32>();
        vec![(Some(self.template.words.get(9).copied().unwrap_or(0) as usize),(0..n).flat_map(|i| {let b=i*5;[b,b+2,b+1,b,b+3,b+2,b,b+4,b+3,b,b+1,b+4]}).collect(),n as usize*5)]
    }
    pub(super) fn blends(&self)->Vec<Blend> {vec![Blend::Additive]}
    #[allow(clippy::too_many_arguments)]
    pub(super) fn vertices(&mut self,time:f32,camera:Vec3,_right:Vec3,_up:Vec3,_gc:&mut R250,_ds:&mut R250,_crt:&mut ao_formats::character::CrtRand)->Result<Option<Vec<Vec<Vertex>>>> {
        if time>=self.duration {return Ok(None);}
        let dt=(time-self.previous).max(0.0);self.previous=time;
        let t=&self.template;
        if t.words.first().copied().unwrap_or(0) & 0x800!=0 {self.position=self.source.w_axis.truncate();}
        self.angle+=t.float(13)?*dt;
        let phase=(time%self.duration)/self.duration;
        let h=self.curves[0].scalar(phase);let w=self.curves[1].scalar(phase);let d=self.curves[2].scalar(phase);
        let bottom=self.curves[3].color(phase);let top=self.curves[4].color(phase);
        let rotation=Mat4::from_cols(self.source.x_axis.truncate().normalize_or_zero().extend(0.0),self.source.y_axis.truncate().normalize_or_zero().extend(0.0),self.source.z_axis.truncate().normalize_or_zero().extend(0.0),self.position.extend(1.0))*Mat4::from_rotation_y(-self.angle);
        let view=rotation.inverse().transform_vector3((camera-self.position).normalize_or_zero());
        let mut vertices=Vec::with_capacity((14..17).map(|i|t.words.get(i).copied().unwrap_or(0)).sum::<u32>() as usize*5);
        // Retail order is X, Z, Y; each plane is a center plus four fan corners.
        for axis in [0,2,1] {
            let count=t.word(14+axis)?;
            for i in 0..count {
                let u=i as f32/count as f32;
                let points=match axis {
                    0=>{let x=u*w-w*0.5;[Vec3::new(x,h*0.5,0.0),Vec3::new(x,0.0,d*0.5),Vec3::new(x,0.0,-w*0.5),Vec3::new(x,h,-d*0.5),Vec3::new(x,h,d*0.5)]},
                    2=>{let z=-(u*w-w*0.5);[Vec3::new(0.0,h*0.5,z),Vec3::new(-w*0.5,0.0,z),Vec3::new(-d*0.5,h,z),Vec3::new(d*0.5,h,z),Vec3::new(w*0.5,0.0,z)]},
                    _=>{let y=h*u;[Vec3::new(0.0,y,0.0),Vec3::new(-w*0.5,y,-w*0.5),Vec3::new(-w*0.5,y,w*0.5),Vec3::new(w*0.5,y,w*0.5),Vec3::new(w*0.5,y,-w*0.5)]}
                };
                let colors=match axis {0=>[mix(bottom,top,0.5),bottom,bottom,top,top],2=>[mix(bottom,top,0.5),bottom,top,top,bottom],_=>[mix(bottom,top,u);5]};
                let uv=if axis==1 || t.words.first().copied().unwrap_or(0) & 0x400!=0 {
                    if axis==0 {[[0.5,0.5],[0.0,0.0],[1.0,0.0],[1.0,1.0],[0.0,1.0]]}
                    else {[[0.5,0.5],[0.0,0.0],[0.0,1.0],[1.0,1.0],[1.0,0.0]]}
                }else if axis==0 {[[0.5,u],[0.0,u],[1.0,u],[1.0,u],[0.0,u]]}else{[[0.5,u],[0.0,u],[0.0,u],[1.0,u],[1.0,u]]};
                let normal=(points[1]-points[0]).normalize_or_zero().cross((points[2]-points[0]).normalize_or_zero());
                let fade=if t.words.first().copied().unwrap_or(0) & 0x4000!=0 && view!=Vec3::ZERO && normal!=Vec3::ZERO {normal.dot(view).abs()}else{1.0};
                for j in 0..5 {let mut color=colors[j];color[3]=(color[3]*255.0*fade).trunc()/255.0;vertices.push(Vertex {pos:rotation.transform_point3(points[j]).to_array(),color:render(color),uv:uv[j],..Default::default()});}
            }
        }
        Ok(Some(vec![vertices]))
    }
}

/// Shield2 duplicates the live posed CAT surface, not its bounding box.
/// GC10110f3a/10110bbf/10111147; DS1001dc74/1001d569.
pub(super) struct ShieldEffect {template:Template,color:Curve,offset:Curve,duration:f32,elapsed:f32,stop:Option<f32>,surface_stopped:bool,stop_child:i32}
impl ShieldEffect {
    pub(super) fn new(t:&Template,_config:EffectConfig)->Result<Self> {
        ensure!(t.kind==3034,"not native Shield2");
        for i in [8,13,14,15,16,17,18] {t.float(i)?;}
        ensure!(t.word(9)?<=1 && t.word(10)?<=2 && t.word(11)?<=2,"invalid native Shield2 deformation/UV/alpha mode");
        ensure!(t.word(20)?==u32::MAX || t.word(20)?<10,"invalid native Shield2 mech resource");
        let mut at=21;let color=Curve::parse(t,&mut at,true)?;let offset=Curve::parse(t,&mut at,false)?;
        let stop_child=t.words.get(at).copied().unwrap_or(0) as i32;
        // Slot9 SetDuration is a native no-op. Slot8 SetColor is not used by this path.
        Ok(Self {template:t.clone(),color,offset,duration:t.float(8)?,elapsed:0.0,stop:None,surface_stopped:false,stop_child})
    }
    pub(super) fn graceful(&mut self) {if self.stop.is_none() {self.stop=Some(self.elapsed);}}
    pub(super) fn frame(&mut self,dt:f32)->bool {
        self.elapsed+=dt;
        if self.stop.is_some_and(|s|self.elapsed-s>2.0) {return false;}
        if self.duration>0.0 && self.elapsed>self.duration-2.0 {self.graceful();}
        true
    }
    pub(super) fn material(&self)->usize {self.template.words.get(12).copied().unwrap_or(0) as usize}
    pub(super) fn uses_source_material(&self)->bool {self.template.words.first().copied().unwrap_or(0) & 0x10000!=0 && self.mech_resource().is_none()}
    pub(super) fn priority(&self)->Option<i32> {if self.template.words.first().copied().unwrap_or(0) & 0x8000!=0 {Some(3)}else{None}}
    pub(super) fn blend(&self)->Blend {if self.template.words.first().copied().unwrap_or(0) & 0x400!=0 {Blend::Additive}else{Blend::AlphaBlend}}
    pub(super) fn layer_count(&self)->usize {self.template.words.get(19).copied().unwrap_or(0) as usize}
    pub(super) fn mech_resource(&self)->Option<&'static str> {
        const RESOURCES:[&str;10]=["EP03_preorder_mech_simple.abiff","EP03_scout_mech_simple.abiff","EP03_heavy_mech_simple.abiff","EP03_anti_personnel_gun_simple.abiff","EP03_anti_vehicle_gun_simple.abiff","EP03_preorder_mech_upgraded_simple.abiff","EP03_scout_mech_upgraded_simple.abiff","EP03_heavy_mech_upgraded_simple.abiff","EP03_anti_personnel_gun_upgraded_simple.abiff","EP03_anti_vehicle_gun_upgraded_simple.abiff"];
        RESOURCES.get(self.template.words.get(20).copied().unwrap_or(0) as usize).copied()
    }
    /// GC10111147 second CAT callback -> DS1001dffe Stop; flags20000 hide it.
    pub(super) fn stop_surface(&mut self) {self.surface_stopped=true;}
    pub(super) fn is_surface_stopped(&self)->bool {self.surface_stopped}
    pub(super) fn take_stop_child(&mut self)->i32 {
        if self.surface_stopped {std::mem::take(&mut self.stop_child)}else{0}
    }
    pub(super) fn vertices(&self,source:&[Vertex],body_scale:f32)->Vec<Vertex> {
        let t=&self.template;let phase=self.elapsed/self.duration;
        if self.surface_stopped && t.words.first().copied().unwrap_or(0) & 0x20000!=0 {return Vec::new();}
        if t.words.first().copied().unwrap_or(0) & 0x40000!=0 && source.len()>1000 {return Vec::new();}
        let offset=self.offset.scalar(phase);let color=self.color.color(phase);
        let frequency=f32::from_bits(t.words.get(18).copied().unwrap_or(0));
        let u=f32::from_bits(t.words.get(13).copied().unwrap_or(0));let vv=f32::from_bits(t.words.get(14).copied().unwrap_or(0));
        let du=f32::from_bits(t.words.get(15).copied().unwrap_or(0))*phase;let dv=f32::from_bits(t.words.get(16).copied().unwrap_or(0))*phase;
        let surface=source.iter().map(|v| {
            let mut out=*v;let p=Vec3::from_array(v.pos);let n=Vec3::from_array(v.normal);
            let displacement=if t.words.get(9).copied().unwrap_or(0) == 1 {(phase*30.0+p.x*2.0+p.y*3.0-p.z).sin().powi(2)*offset}else{offset};
            out.pos=((p+n*displacement)*body_scale).to_array();
            out.uv=match t.words.get(10).copied().unwrap_or(0) { 0=>[(v.uv[0]+du)*u,(v.uv[1]+dv)*vv],
            1=>[p.x.atan2(-p.z)*u/std::f32::consts::TAU+du,p.y*vv+dv],
            _=>[p.x*u+du,p.y*vv+dv], };
            let z=-p.z;let f=phase*frequency;
            let mut alpha=1.0;
            if t.words.get(11).copied().unwrap_or(0)!=0 {
                alpha=((p.y*2.5+p.x*4.0+z*3.0+f*1.1)*2.5).cos()*((p.y*3.0+p.x*2.5+z*4.0+f)*2.3).sin()*((p.y*4.0+p.x*3.0+z*1.5+f*1.2)*1.5).sin();
                if t.words.get(11).copied().unwrap_or(0) == 2 {alpha*=phase*1.5;}
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
                if t.words.first().copied().unwrap_or(0) & 0x1000!=0 {p.y-=expand;}
                v.pos=p.to_array();out.push(v);
            }
        }
        out
    }
}

#[derive(Clone,Copy)]
struct TrailSample {matrix:Mat4,color:[f32;4]}
/// GC10114c39/101150b4/10114bae; DS1002c7bd/1002c918/1002d169.
pub(super) struct TrailEffect {template:Template,source:Mat4,origin:TrailSample,samples:Vec<TrailSample>,head:usize,count:usize,curves:[Curve;3],previous:f32,emission:f32,done:bool,vehicle_direction:Option<Vec3>}
impl TrailEffect {
    pub(super) fn new(t:&Template,source:Mat4,_target:Mat4,_color:u32,_gc:&mut R250,_ds:&mut R250,_crt:&mut ao_formats::character::CrtRand)->Result<Self> {
        ensure!(t.kind==3039,"not native Trail2");
        ensure!((1..=4096).contains(&t.word(11)?),"invalid Trail2 sample capacity");
        for i in [1,2,3,4,5,6,8,12,13,14,15] {t.float(i)?;}
        super::materials::MATERIALS.get(t.words.get(10).copied().unwrap_or(0) as usize).ok_or_else(||anyhow::anyhow!("unknown Trail2 material"))?;
        let mut at=16;let curves=[Curve::parse(t,&mut at,true)?,Curve::parse(t,&mut at,false)?,Curve::parse(t,&mut at,false)?];
        let source=super::sprites::connector(t,source)?;
        let origin=TrailSample {matrix:source,color:[0.0;4]};
        Ok(Self {template:t.clone(),source,origin,samples:vec![origin;t.word(11)? as usize],head:0,count:0,curves,previous:0.0,emission:0.0,done:false,vehicle_direction:None})
    }
    pub(super) fn configure(&mut self,_config:EffectConfig)->Result<()> {Ok(())}
    pub(super) fn update_source(&mut self,source:Mat4)->Result<()> {self.source=super::sprites::connector(&self.template,source)?;Ok(())}
    pub(super) fn set_vehicle_direction(&mut self,direction:Vec3) {self.vehicle_direction=Some(direction);}
    pub(super) fn graceful(&mut self) {self.done=true;}
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {
        let n=self.samples.len()+1;
        let indices=(0..4u32).flat_map(|strip|(0..n as u32-1).flat_map(move |i|{let b=strip*n as u32*2+i*2;[b,b+1,b+2,b+1,b+3,b+2]})).collect();
        vec![(Some(self.template.words.get(10).copied().unwrap_or(0) as usize),indices,n*8)]
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
        let phase=(time*f32::from_bits(self.template.words.get(13).copied().unwrap_or(0)))%1.0;
        let mut matrix=self.source;
        matrix.x_axis*=self.curves[1].scalar(phase);matrix.y_axis*=self.curves[2].scalar(phase);
        let movement=matrix.w_axis.truncate()-self.origin.matrix.w_axis.truncate();
        let mut color=self.curves[0].color(phase);
        let mut emit=true;
        if self.template.words.first().copied().unwrap_or(0) & 0x800!=0 {
            if movement==Vec3::ZERO {emit=false;}
            else {
                let direction=self.vehicle_direction.ok_or_else(||anyhow::anyhow!("Trail2 requires source vehicle direction"))?;
                if movement.normalize().dot(direction.normalize_or_zero())<=-0.75 {color=[0.0;4];}
            }
        }
        if emit {
            self.origin=TrailSample {matrix,color};
            let period=f32::from_bits(self.template.words.get(12).copied().unwrap_or(0));
            if period<=0.0 {self.sample(self.origin);}else{
                self.emission+=dt;
                while self.emission>=period {self.sample(self.origin);self.emission-=period;}
            }
        }
        if self.template.words.first().copied().unwrap_or(0) & 0x1000!=0 && dt!=0.0 {
            let thrust=(super::random_fraction(ds)*f32::from_bits(self.template.words.get(15).copied().unwrap_or(0))+f32::from_bits(self.template.words.get(14).copied().unwrap_or(0)))*dt;
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
                    let uv=if self.template.words.first().copied().unwrap_or(0) & 0x400==0 {[u,side as f32]}else{[side as f32,u]};
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
