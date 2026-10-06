//! Authored BuffPlaceHolder (1002), Cord (1003), and recursive tracers (1022/3026).
//! GC: dispatch 100ce4f7; 1002 load/new/process 100d4f72/100d53e3/100d4d6b;
//! 1003 load/new/process/update 100d5b7d/100d5ec8/100d5a02/100d6480;
//! 1022 load/new/process 100ff101/100ff18c/100ff1fe.
//! 3026 load/new/process/finish 10114717/10114932/10114622/101145f1.
//! Meta group (2007): 100e59b2/100e5a25/100e57f3, destructor 100e5799.
use super::{Binding, EffectConfig, Renderer, Template};
use anyhow::{ensure, Result};
use ao_scene::{Blend, Vertex};
use glam::{Mat4, Vec3};
use std::collections::VecDeque;

const LINKS: usize = 256;
// GC double 10167f88 = 0.44999998807907104 (exact f32 0.45 promoted to double).
const ANGLE_STEP: f32 = 0.45;

#[derive(Clone, Copy)]
struct Link { point: Vec3, velocity: Vec3, remaining: f32, color: [f32; 4] }

pub struct Composition {
    template: Template,
    source: Mat4,
    target: Vec3,
    config: EffectConfig,
    children: [u32; 10],
    elapsed: f32,
    duration: f32,
    angle: f32,
    sampled_angle: f32,
    stopped: bool,
    links: VecDeque<Link>,
    origin: Vec3,
    head: Vec3,
    tracer_speed: f32,
    finished: bool,
}

fn rgba(t: &Template, at: usize) -> Result<[f32; 4]> {
    Ok([t.float(at+1)?, t.float(at+2)?, t.float(at+3)?, t.float(at)?])
}


// GC100d57bb/100d624c select the profile by breed, female sex code 3, body shape.
fn body_profile(profile:&Template, appearance:[i32;4])->Result<Option<(f32,f32)>> {
    let [breed,sex,shape,_]=appearance;
    if !(1..=4).contains(&breed) {return Ok(None);}
    ensure!((0..=2).contains(&shape),"invalid authored body shape {shape}");
    let i=((breed-1)*12+if sex==3 {6} else {0}+shape*2) as usize;
    Ok(Some((profile.float(i)?,profile.float(i+1)?)))
}

// GC1013c1f7, conjugated by the native-to-scene Z reflection. The original
// leaves zero X/Y axes for a vertical nonzero direction; no invented up vector.
fn tracer_matrix(direction:Vec3, point:Vec3)->Mat4 {
    let z=direction.normalize_or_zero();
    let x=-z.cross(Vec3::Y).normalize_or_zero();
    if x==Vec3::ZERO && z==Vec3::ZERO {return Mat4::from_translation(point);}
    let y=-z.cross(x);
    Mat4::from_cols(x.extend(0.0),y.extend(0.0),(-z).extend(0.0),point.extend(1.0))
}

// CMS integer accessor 10106872 returns zero outside the authored payload. Meta
// has ten ID slots, not the shared connector header; slot9=-1 is a lifetime flag.
fn meta_ids(t:&Template)->[i32;10] {
    let mut ids=std::array::from_fn(|i|t.words.get(i).copied().unwrap_or(0) as i32);
    if ids[9]==-1 {ids[9]=0;}
    ids
}
impl Composition {
    pub fn supports(kind: i32) -> bool { matches!(kind, 1002 | 1003 | 1022 | 2007 | 3026) }

    pub fn new(t: &Template, source: Mat4, target: Vec3, config: EffectConfig) -> Result<Self> {
        ensure!(Self::supports(t.kind), "unsupported composition class {}", t.kind);
        if t.kind==2007 {
            let duration=config.duration.map(|d|d+15.0).unwrap_or_else(||if t.words.get(9)==Some(&u32::MAX) {-1.0} else {90.0});
            let origin=source.w_axis.truncate();
            return Ok(Self {template:t.clone(),source,target,config,children:[0;10],elapsed:0.0,duration,
                angle:0.0,sampled_angle:0.0,stopped:false,links:VecDeque::new(),origin,head:origin,tracer_speed:0.0,finished:false});
        }
        let end = match t.kind { 1002 => 26, 1003 => 36, 3026=>14, _ => 15 };
        t.word(end)?;
        for i in 1..=6 { t.float(i)?; }
        if t.kind == 1002 {
            for i in 12..=25 { t.float(i)?; }
        } else if t.kind == 1003 {
            for i in 10..=35 { if !matches!(i,24|31) { t.float(i)?; } }
            ensure!(t.float(35)? > 0.0, "invalid cord link lifetime");
            rgba(t,16)?;
        } else if t.kind==1022 {
            ensure!(t.float(10)? > 0.0, "invalid moving tracer speed");
            rgba(t,11)?;
        } else {
            ensure!(t.float(12)? > 0.0,"invalid recursive tracer speed");
            t.float(13)?;
        }
        let duration = config.duration.unwrap_or(t.float(8)?);
        let angle = if t.kind == 1002 { t.float(22)? } else { 0.0 };
        let origin=source.w_axis.truncate();
        let tracer_speed=if t.kind==3026 {t.float(12)?.min((target-origin).length()*5.0)} else {0.0};
        let source=if t.kind==3026 {Mat4::from_translation(origin)} else {source};
        Ok(Self { template:t.clone(), source, target, config, children:[0;10], elapsed:0.0,
            duration, angle, sampled_angle:angle, stopped:false, links:if t.kind==1003 {VecDeque::with_capacity(LINKS)} else {VecDeque::new()},
            origin,head:origin,tracer_speed,finished:false })
    }

    // Initialize while Renderer still holds the template-id recursion path. Deferred spawning
    // would allow an authored A->B->A cycle to evade the shared cycle/depth check.
    pub fn initialize(&mut self, renderer: &mut Renderer) -> Result<()> {
        let profile_index=match self.template.kind {1002=>Some(26),1003=>Some(36),_=>None};
        if let Some(index)=profile_index {
            let profile_id=self.template.word(index)? as i32;
            if profile_id!=0 && self.config.source_identity.is_some() {
                let appearance=self.config.source_appearance.ok_or_else(||anyhow::anyhow!("missing dynel appearance for effect body profile {profile_id}"))?;
                let profile=renderer.templates.by_id.get(&profile_id).ok_or_else(||anyhow::anyhow!("missing effect body profile {profile_id}"))?;
                ensure!(profile.kind==0,"invalid effect body profile class {}",profile.kind);
                if let Some((z,radius))=body_profile(profile,appearance)? {
                    self.template.words[3]=z.to_bits();
                    if self.template.kind==1002 {self.template.words[21]=radius.to_bits();}
                }
                if self.template.kind==1002 {
                    self.config.scale=Some(appearance[3] as f32*0.01);
                }
            }
        }
        let t=&self.template;
        let mut ids=[0;10];
        match t.kind {
            1002=> {ids[0]=t.word(10)? as i32;ids[1]=t.word(11)? as i32;}
            1022=>ids[0]=t.word(15)? as i32,
            3026=>ids[0]=t.word(11)? as i32,
            2007=>ids=meta_ids(t),
            _=>return Ok(()),
        }
        let start = if t.kind==1002 {self.config.start_color.unwrap_or(rgba(t,12)?)} else {[0.0;4]};
        let stop = if t.kind==1002 {self.config.stop_color.unwrap_or(rgba(t,16)?)} else {[0.0;4]};
        for (i,id) in ids.into_iter().enumerate() {
            if id==0 { continue; }
            let (source,config)=if t.kind==1022 {
                let color=self.config.start_color.unwrap_or(rgba(t,11)?);
                let mut stop=color;stop[3]=0.0;
                (self.source,EffectConfig {start_color:Some(color),stop_color:Some(stop),..Default::default()})
            } else if t.kind==3026 {
                (self.source,EffectConfig {start_color:Some(self.config.start_color.unwrap_or([1.0;4])),stop_color:Some(self.config.stop_color.unwrap_or([0.0;4])),..Default::default()})
            } else if t.kind==2007 {
                // Meta preserves its overload: GC100e5a90 forwards the matrix,
                // GC100e5b66 forwards the dynel and explicit connector unchanged.
                (self.source,self.config)
            } else {
                let source=if i==0 { Mat4::from_translation(self.orbit_position(self.angle,true)?) } else {self.source};
                (source,EffectConfig {start_color:Some(start),stop_color:Some(stop),source_identity:if i==1 {self.config.source_identity} else {None},source_appearance:self.config.source_appearance,..Default::default()})
            };
            let binding=Binding {group:0,attractor:0,effect:id,note:0,color:0};
            match renderer.spawn_configured(binding,source,self.target,config) {
                Ok(handle)=>self.children[i]=handle,
                Err(error)=> {
                    return Err(match self.cancel(renderer) {Ok(())=>error,Err(cancel)=>error.context(format!("effect cancellation also failed: {cancel:#}"))});
                }
            }
        }
        Ok(())
    }

    pub fn forwarded_children(&self)->Option<[u32;10]> {
        (self.template.kind==2007).then_some(self.children)
    }

    fn anchor(&self) -> Result<Mat4> {super::sprites::connector(&self.template,self.source)}

    fn orbit_position(&self, angle:f32, initial:bool)->Result<Vec3> {
        let a=self.anchor()?;
        let radius=self.template.float(21)?;
        // Initial constructor uses E2; subsequent samples use E1 (GC100d52dc/100d4d6b).
        let axis=if initial {-a.z_axis.truncate()} else {a.y_axis.truncate()};
        Ok(a.w_axis.truncate()+radius*(a.x_axis.truncate()*angle.cos()+axis*angle.sin()))
    }

    pub fn update_source(&mut self, source:Mat4) {self.source=source;}

    pub fn update_position(&mut self, point:Vec3) {
        if self.template.kind!=1003 {self.source.w_axis=point.extend(1.0);return;}
        if self.template.kind==1003 && !self.stopped && (self.duration<0.0 || self.elapsed<self.duration-f32::from_bits(self.template.words[35])) {
            // GC100d6480: position-only connector appends a link for every update, even
            // repeated positions; the 256-entry circular list is the native bounded pool.
            if self.links.len()==LINKS {self.links.pop_front();}
            let color=self.config.start_color.unwrap_or_else(|| {
                let w=&self.template.words;
                [f32::from_bits(w[17]),f32::from_bits(w[18]),f32::from_bits(w[19]),f32::from_bits(w[16])]
            });
            self.links.push_back(Link {point,velocity:point,remaining:f32::from_bits(self.template.words[35]),color});
        }
    }

    pub fn next_state(&mut self) {
        if self.template.kind==2007 {return;}
        self.stopped=true;
        self.duration=if self.template.kind==1003 {self.elapsed+f32::from_bits(self.template.words[35])} else {self.elapsed};
    }
    pub fn cancel(&mut self, renderer:&mut Renderer)->Result<()> {
        // GC101145f1: the impact template is created by the destructor, including
        // explicit cancellation and timeout, at the current (possibly partial) head.
        let result=if self.template.kind==3026 && !self.finished {
            self.finished=true;
            let id=self.template.word(14)? as i32;
            if id==0 {Ok(())} else {
                renderer.spawn_configured(Binding {group:0,attractor:0,effect:id,note:0,color:0},
                    Mat4::from_translation(self.head),self.head,EffectConfig::default()).map(|_|())
            }
        } else {Ok(())};
        for child in &mut self.children {if *child!=0 {renderer.delete(*child);*child=0;}}
        result
    }

    pub fn frame(&mut self, dt:f32, renderer:&mut Renderer)->Result<bool> {
        ensure!(dt.is_finite() && dt>=0.0,"invalid composition delta");
        self.elapsed+=dt;
        if self.duration>=0.0 && self.elapsed>=self.duration {self.cancel(renderer)?;return Ok(false);}
        match self.template.kind {
            2007 => {
                if !self.children.iter().any(|&handle|handle!=0 && renderer.is_active(handle)) {self.cancel(renderer)?;return Ok(false);}
            }
            1002 => {
                if self.children[1]!=0 {renderer.update_source(self.children[1],self.source);}
                self.angle+=self.template.float(23)?*dt;
                let count=((self.angle-self.sampled_angle)/ANGLE_STEP).trunc().max(0.0);
                ensure!(count<=65536.0,"composition angular update exceeds bounded work");
                for _ in 0..count as usize {
                    self.sampled_angle+=ANGLE_STEP;
                    if self.children[1]!=0 {
                        let r=self.template.float(21)?*self.config.scale.unwrap_or(1.0);
                        let local=Vec3::new(self.sampled_angle.cos()*r,0.0,-self.sampled_angle.sin()*r);
                        renderer.update_position(self.children[1],local);
                    }
                    if self.children[0]!=0 {renderer.update_position(self.children[0],self.orbit_position(self.sampled_angle,false)?);}
                }
            }
            1003 => {
                // GC100d6480 passes the incoming position as both link point and velocity.
                for link in &mut self.links {link.point+=link.velocity*dt.clamp(0.01,0.025);link.remaining-=dt;}
                self.links.retain(|l|l.remaining>0.0);
            }
            1022 => {
                let delta=self.target-self.source.w_axis.truncate();
                let distance=delta.length();
                let speed=self.template.float(10)?.min(distance*5.0);
                let travel=speed*self.elapsed;
                if travel>=distance {self.cancel(renderer)?;return Ok(false);}
                let point=self.source.w_axis.truncate()+delta.normalize_or_zero()*travel;
                renderer.update_position(self.children[0],point);
            }
            3026 => {
                let delta=self.target-self.origin;
                let distance=delta.length();
                let travel=self.tracer_speed*self.elapsed;
                self.head=self.origin+delta.normalize_or_zero()*travel.min(distance);
                renderer.update_source(self.children[0],tracer_matrix(delta,self.head));
                if travel>distance {self.cancel(renderer)?;return Ok(false);}
            }
            _=>unreachable!(),
        }
        Ok(true)
    }

    pub fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {
        if self.template.kind!=1003 {return Vec::new();}
        let indices=(0..LINKS as u32-1).flat_map(|i| {let n=i*2;[n,n+1,n+2,n+1,n+2,n+3]}).collect();
        vec![(Some(self.template.words[9] as usize),indices,LINKS*2)]
    }
    pub fn blends(&self)->Vec<Blend> {if self.template.kind==1003 {vec![Blend::Additive]} else {Vec::new()}}

    pub fn vertices(&mut self,_time:f32,camera:Vec3,_right:Vec3,_up:Vec3)->Result<Option<Vec<Vec<Vertex>>>> {
        if self.template.kind!=1003 {return Ok(None);}
        let mut vertices=vec![Vertex {color:[0.0;4],..Default::default()};LINKS*2];
        let life=self.template.float(35)?;
        let width=self.template.float(12)?;
        // DS1000e3e8/1000e7d0: shared ribbon vertices with averaged adjacent sides.
        let mut previous=Vec3::ZERO;
        let transform=if self.config.source_identity.is_some() {self.anchor()?} else {Mat4::IDENTITY};
        for i in 0..self.links.len() {
            let index=self.links.len()-1-i;
            let link=self.links[index];
            let next=index.checked_sub(1).and_then(|j|self.links.get(j)).copied().unwrap_or(link);
            let point=transform.transform_point3(link.point);
            let next_point=transform.transform_point3(next.point);
            let side=(next_point-point).cross(camera-point).normalize_or_zero();
            let offset=if i==0 {side*width} else if i+1==self.links.len() {previous*width} else {(previous+side)*0.5*width};
            previous=side;
            let mut color=link.color;color[3]*=(link.remaining/life).clamp(0.0,1.0);
            for (j,sign) in [-1.0,1.0].into_iter().enumerate() {
                vertices[i*2+j]=Vertex {pos:(point+offset*sign).to_array(),normal:[0.0,1.0,0.0],uv:[j as f32,1.0],color};
            }
        }
        if let Some(last)=self.links.front() {
            for vertex in &mut vertices[self.links.len()*2..] {vertex.pos=transform.transform_point3(last.point).to_array();vertex.color=[0.0;4];}
        }
        Ok(Some(vec![vertices]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn template(kind:i32,n:usize)->Template {Template {kind,words:vec![0;n]}}
    #[test]
    fn authored_ids_and_orbit_axes_are_not_substituted() {
        let mut t=template(1002,27);t.words[8]=1.5f32.to_bits();t.words[10]=6203;t.words[11]=20012;
        t.words[21]=0.15f32.to_bits();t.words[22]=std::f32::consts::FRAC_PI_2.to_bits();
        let c=Composition::new(&t,Mat4::IDENTITY,Vec3::ZERO,EffectConfig::default()).unwrap();
        assert_eq!(c.template.words[11],20012);
        assert!((c.orbit_position(c.angle,true).unwrap()+Vec3::Z*0.15).length()<1e-6);
        assert!((c.orbit_position(c.angle,false).unwrap()-Vec3::Y*0.15).length()<1e-6);
        t.words.pop();assert!(Composition::new(&t,Mat4::IDENTITY,Vec3::ZERO,EffectConfig::default()).is_err());
    }
    #[test]
    fn cord_updates_keep_native_pool_and_stop_tail() {
        let mut t=template(1003,37);t.words[8]=(-1.0f32).to_bits();t.words[35]=0.5f32.to_bits();
        let mut c=Composition::new(&t,Mat4::IDENTITY,Vec3::ZERO,EffectConfig::default()).unwrap();
        for i in 0..300 {c.update_position(Vec3::X*i as f32);}
        assert_eq!(c.links.len(),256);assert_eq!(c.links[0].point.x,44.0);
        c.next_state();c.update_position(Vec3::ZERO);assert_eq!(c.links.len(),256);assert_eq!(c.duration,0.5);
    }
    #[test]
    fn body_profiles_select_authored_breed_sex_and_shape() {
        let mut profile=template(0,42);
        for (i,w) in profile.words.iter_mut().enumerate() {*w=(i as f32).to_bits();}
        assert_eq!(body_profile(&profile,[1,2,0,100]).unwrap(),Some((0.0,1.0)));
        assert_eq!(body_profile(&profile,[1,3,2,100]).unwrap(),Some((10.0,11.0)));
        assert_eq!(body_profile(&profile,[3,2,1,100]).unwrap(),Some((26.0,27.0)));
        assert_eq!(body_profile(&profile,[4,2,2,100]).unwrap(),Some((40.0,41.0)));
        assert!(body_profile(&profile,[4,3,0,100]).is_err());
    }
    #[test]
    fn recursive_tracer_uses_authored_child_speed_and_native_basis() {
        let mut t=template(3026,15);t.words[8]=60.0f32.to_bits();t.words[11]=71520;t.words[12]=40.0f32.to_bits();t.words[14]=71004;
        let c=Composition::new(&t,Mat4::IDENTITY,Vec3::Z,EffectConfig::default()).unwrap();
        assert_eq!(c.template.words[11],71520);
        assert_eq!(c.tracer_speed,5.0);
        let m=tracer_matrix(Vec3::Z,Vec3::X);
        assert_eq!(m.x_axis.truncate(),Vec3::X);
        assert_eq!(m.y_axis.truncate(),-Vec3::Y);
        assert_eq!(m.z_axis.truncate(),-Vec3::Z);
        assert_eq!(m.w_axis.truncate(),Vec3::X);
        let vertical=tracer_matrix(Vec3::Y,Vec3::ZERO);
        assert_eq!(vertical.x_axis.truncate(),Vec3::ZERO);
        assert_eq!(vertical.y_axis.truncate(),Vec3::ZERO);
    }
    #[test]
    fn meta_payload_is_actual_ids_not_connector_header() {
        let t=Template {kind:2007,words:vec![71340,71341,71342,71343,71344,71345,0,0,0,0]};
        assert_eq!(meta_ids(&t),[71340,71341,71342,71343,71344,71345,0,0,0,0]);
        assert_eq!(Composition::new(&t,Mat4::IDENTITY,Vec3::ZERO,EffectConfig::default()).unwrap().duration,90.0);
        let short=Template {kind:2007,words:vec![2652,0,0,0,0,0,0,0,0]};
        assert_eq!(meta_ids(&short)[9],0);
        let indefinite=Template {kind:2007,words:vec![12541,12542,12543,12544,0,0,0,0,0,u32::MAX]};
        assert_eq!(meta_ids(&indefinite)[9],0);
        assert_eq!(Composition::new(&indefinite,Mat4::IDENTITY,Vec3::ZERO,EffectConfig::default()).unwrap().duration,-1.0);
    }
}
