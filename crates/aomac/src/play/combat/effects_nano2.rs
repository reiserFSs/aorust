//! 1017: GC load 100e8c4f/init 100e8cf2/process 100e88bb;
//! DS ctor 10018c08/process 1001975e/render 10018de7. No sprite replacement.
use super::Template;
use anyhow::{ensure, Result};
use ao_formats::character::CrtRand;
use ao_formats::weather::R250;
use ao_scene::{Blend, Vertex};
use glam::{Mat4, Vec3};

pub(super) const ANCHORS: [i32; 12] = [1006,1007,1008,1009,1010,2001,2000,1011,1012,1013,1014,1000];
const ORDER: [usize;18] = [0,0,1,1,3,3,2,2,4,4,5,5,7,7,6,6,8,8];
#[derive(Default)]
pub(super) struct Nano2State { phase:f32, cycle:u32 }
pub(super) struct Nano2 {
    material:usize, duration:f32, previous:f32, anchors:[Vec3;12],
}
impl Nano2 {
    pub(super) fn new(template:&Template)->Result<Self> {
        ensure!(template.kind==1017,"not Nano2 template");
        let material=template.word(9)? as usize;
        ensure!(material<super::materials::MATERIALS.len(),"invalid Nano2 material");
        for i in 10..18 { template.float(i)?; }
        Ok(Self { material,duration:template.float(8)?,previous:0.0,anchors:[Vec3::ZERO;12] })
    }
    pub(super) fn set_duration(&mut self,duration:f32) { self.duration=duration; }
    // GC initializes each missing rig anchor to zero; never substitute actor bounds.
    pub(super) fn refresh_anchors(&mut self,mut get:impl FnMut(i32)->Option<Mat4>) {
        for (position,id) in self.anchors.iter_mut().zip(ANCHORS) {
            *position=get(id).map_or(Vec3::ZERO,|m|m.w_axis.truncate());
        }
    }
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {
        let indices=(0..18u32).flat_map(|trail|(0..9u32).flat_map(move |segment| {
            let a=trail*20+segment*2; [a,a+1,a+2,a+1,a+2,a+3]
        })).collect();
        vec![(Some(self.material),indices,360)]
    }
    pub(super) fn blends(&self)->Vec<Blend> { vec![Blend::Additive] }
    pub(super) fn vertices(&mut self,time:f32,view:[Vec3;3],state:&mut Nano2State,ds:&mut R250,crt:&mut CrtRand)->Result<Option<Vec<Vec<Vertex>>>> {
        let [camera,right,up]=view;
        ensure!(time.is_finite() && time>=self.previous,"invalid Nano2 time");
        if self.duration>=0.0 && time>=self.duration { return Ok(None); }
        let dt=time-self.previous; self.previous=time;
        // These globals advance once per ProcessStuff call, not once per frame.
        *crt=CrtRand::new(42);
        state.phase+=dt*5.0;
        if state.phase>1.0 { state.phase=0.0; state.cycle=(state.cycle+1)%5; }
        // DS time() reseeds all eighteen blocks identically in wall-clock seconds.
        let seed=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs() as u32;
        let (_,columns,rows,_,_)=super::materials::MATERIALS[self.material];
        let mut vertices=Vec::with_capacity(360);
        for (trail,anchor) in ORDER.into_iter().enumerate() {
            *crt=CrtRand::new(seed);
            let start=self.anchors[anchor];
            let direction=(start-self.anchors[11]).normalize_or_zero()*2.0;
            // Preserve DS's 200-byte block aliasing: the eleventh position overlaps
            // widths, and the eleventh width overlaps color[0]. Render consumes ten.
            let mut block=[0u32;51];
            for point in 0..11 {
                let t=point as f32*0.125; let fade=1.0-t;
                let random=Vec3::new(unit(ds)*2.0-1.0,unit(ds)*2.0-1.0,1.0-unit(ds)*2.0).normalize_or_zero();
                let position=start+direction*(state.phase*t)+random*(state.phase*0.1*fade);
                for (i,value) in position.to_array().into_iter().enumerate() { block[point*3+i]=value.to_bits(); }
                block[30+point]=((unit(ds)*(fade*0.066-0.033)+0.033)*4.0).to_bits();
                let pulse=if state.cycle==0 { (1.0-state.phase)*255.0 } else {0.0};
                let (r,g)=if trail==0 {(pulse*0.5,pulse*0.3)}else{(pulse*0.3,pulse*0.5)};
                block[40+point]=pack(fade*255.0,r,g,pulse*0.2);
            }
            let points:[Vec3;10]=std::array::from_fn(|i|Vec3::new(f32::from_bits(block[i*3]),f32::from_bits(block[i*3+1]),f32::from_bits(block[i*3+2])));
            let widths:[f32;10]=std::array::from_fn(|i|f32::from_bits(block[30+i]));
            let mut ends=[[[Vec3::ZERO;2];2];9];
            for i in 0..9 {
                let project=|p:Vec3| { let p=p-camera; let depth=p.dot(right.cross(up)); if depth!=0.0 {glam::Vec2::new(p.dot(right),p.dot(up))/depth} else {glam::Vec2::ZERO} };
                let delta=(project(points[i+1])-project(points[i])).normalize_or_zero();
                let tangent=if delta==glam::Vec2::ZERO {right}else{right*delta.x+up*delta.y};
                let side=if delta==glam::Vec2::ZERO {up}else{right*delta.y-up*delta.x};
                // DS1008ade8 shifts half a unit toward the camera.
                let offset=right.cross(up)*-0.5;
                ends[i][0]=[points[i]+offset-tangent*widths[i]-side*widths[i],points[i]+offset-tangent*widths[i]+side*widths[i]];
                ends[i][1]=[points[i+1]+offset+tangent*widths[i+1]-side*widths[i+1],points[i+1]+offset+tangent*widths[i+1]+side*widths[i+1]];
            }
            for i in 0..10 {
                let edge=if i==0 {ends[0][0]} else if i==9 {ends[8][1]} else {std::array::from_fn(|side|(ends[i-1][1][side]+ends[i][0][side])*0.5)};
                let [a,r,g,b]=block[40+i].to_be_bytes();
                let color=[r,g,b].map(|v|(v as f32/255.0).powf(2.2));
                for (side,point) in edge.into_iter().enumerate() { vertices.push(Vertex {pos:point.to_array(),normal:[0.0;3],uv:[side as f32/columns as f32,i as f32/9.0/rows as f32],color:[color[0],color[1],color[2],a as f32/255.0]}); }
            }
        }
        Ok(Some(vec![vertices]))
    }
}
fn unit(ds:&mut R250)->f32 {super::random_fraction(ds)}
fn pack(a:f32,r:f32,g:f32,b:f32)->u32 {
    // x87 conversion is signed before native shifts/ORs, including negative tail alpha.
    let byte=|v:f32|(v-0.5).round_ties_even() as i32 as u32;
    (((byte(a)<<8|byte(r))<<8|byte(g))<<8)|byte(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authored_8012_nano2_trails() {
        let mut words=vec![0;40]; words[0]=1285; words[8]=(-1.0f32).to_bits(); words[9]=15;
        for value in &mut words[11..17] {*value=1.0f32.to_bits();} words[17]=0.8f32.to_bits();
        let template=Template {kind:1017,words};
        let mut visual=Nano2::new(&template).unwrap();
        let mut queried=Vec::new(); visual.refresh_anchors(|id| {queried.push(id);Some(Mat4::from_translation(Vec3::new(id as f32*0.001,1.0,0.0)))});
        assert_eq!(queried,ANCHORS); assert_eq!(visual.models()[0].1.len(),18*9*6);
        let mut state=Nano2State::default(); let mut crt=CrtRand::new(1); let mut ds=R250::new(1);
        let vertices=visual.vertices(0.1,[Vec3::new(0.0,0.0,5.0),Vec3::X,Vec3::Y],&mut state,&mut ds,&mut crt).unwrap().unwrap();
        assert_eq!(vertices[0].len(),360); assert!(vertices[0].iter().all(|v|v.pos.iter().all(|x|x.is_finite())));
        assert_ne!(vertices[0][0].pos,vertices[0][40].pos);
        visual.set_duration(0.2); assert!(visual.vertices(0.2,[Vec3::ZERO,Vec3::X,Vec3::Y],&mut state,&mut ds,&mut crt).unwrap().is_none());
    }
    #[test]
    #[ignore="installed authored Nano2, source actor rig and offscreen GPU; AOMAC_EFFECT_FRAMES required"]
    fn authored_nano2_actor_frames()->Result<()> {
        use super::super::{Renderer,Binding,EffectConfig,Creation,MODEL_BASE};
        use ao_formats::character::{actor::{ActorAssets,ActorRig,PlayerLook},Breed,Gender,Skin,Equipment};
        let out=std::path::PathBuf::from(std::env::var_os("AOMAC_EFFECT_FRAMES").ok_or_else(||anyhow::anyhow!("AOMAC_EFFECT_FRAMES required"))?);
        std::fs::create_dir_all(&out)?;
        let mut r=Renderer::open(&ao_gui::client_dir())?;
        ensure!(r.templates.by_id.get(&8012).is_some_and(|t|t.kind==1017),"missing authored8012");
        let assets=ActorAssets::new(&r.store)?;
        let look=PlayerLook {breed:Breed::Solitus,gender:Gender::Male,skin:Skin::Caucasian,build:1,head:None,equipment:Equipment::default()};
        let rig=ActorRig::player(&r.store,&assets,&look,&[(1,15839)])?;
        let origin=Vec3::new(5000.0,10.0,5000.0);let eye=origin+Vec3::new(4.0,3.0,8.0);let identity=(50000,1);
        let mut host=ao_render::Host::headless();host.camera=ao_render::Camera::look_at(eye,origin+Vec3::Y);
        let handle=r.spawn_configured(Binding {group:0,attractor:0,effect:8012,note:0,color:0},Mat4::from_translation(origin),origin,EffectConfig {source_identity:Some(identity),creation:Creation::Dynel,..Default::default()})?;
        for frame in 1..=60 {
            let (skin,parts)=rig.pose(None);
            let actor=ao_scene::ActorFrame {id:1,model:1,transform:Mat4::from_translation(origin).to_cols_array_2d(),skin:Some(skin),parts,part_attractors:rig.part_attractors(),..Default::default()};
            r.prepare_source_mesh(identity,rig.model(),&actor);
            r.refresh_anchors(|source,id| {
                (source==identity).then(||rig.effect_anchor(id,None)).flatten()
                    .map(|local|Mat4::from_translation(origin)*Mat4::from_cols_array_2d(&local))
            });
            assert!(r.anchors.get(&(identity,1000)).copied().flatten().is_some(),"Nano2 fixture needs the actual posed pelvis anchor");
            host.actors.clear();host.actors.push(actor);
            r.frame(1.0/60.0,&mut host,None);
            if [1,6,12,30,60].contains(&frame) {
                let mut models=vec![(1,rig.model().clone())];
                models.extend(r.models.iter().map(|(&id,m)|(MODEL_BASE|id as u32 as u64,m.scene.clone())));
                ao_render::render_to_png_actors(&ao_scene::Scene::default(),&models,host.actors.clone(),eye.to_array(),(origin+Vec3::Y).to_array(),640,480,&out.join(format!("nano2_8012_{frame}.png")),frame as f32/60.0)?;
            }
        }
        r.delete(handle);assert!(!r.is_active(handle));
        Ok(())
    }
}
