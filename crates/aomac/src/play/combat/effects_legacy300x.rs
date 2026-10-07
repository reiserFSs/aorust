//! GC Mesh3010: constructors100e50e2/100e5151/100e51c0,
//! loader100e4fad, init100e4e96, process100e4d9e, delete100e5232.
use super::{EffectConfig, Template};
use anyhow::{ensure, Context, Result};
use ao_formats::{character::NameTable, mesh::MESH_TYPE};
#[cfg(test)]
use ao_formats::mesh::load_mesh;
use ao_rdb::RecordStore;
use ao_scene::{ActorFrame, Blend, Scene, Vertex};
use glam::{Mat4, Quat, Vec3};
use std::sync::Arc;

pub(super) struct TowerMesh { t:Template, source:Mat4, elapsed:f32, started:bool, stopped:bool, scene:Option<Arc<Scene>>, resource:u32 }
fn tower_name(selector:u32)->Option<&'static str> {match selector {0=>Some("tower_destroyed_buff&debuff.abiff"),1=>Some("tower_destroyed_buff&debuff_LL.abiff"),2=>Some("tower_destroyed_controller.abiff"),3=>Some("tower_destroyed_guard.abiff"),_=>None}}
impl TowerMesh {
    pub(super) fn new(t:&Template,store:&RecordStore,names:&NameTable,source:Mat4,resources:&mut std::collections::HashMap<u32,Arc<Scene>>)->Result<Self> {
        ensure!(t.kind==3010,"not a native Mesh template");
        for i in 8..=13 {if i!=9 {t.float(i)?;}}
        let resource=tower_name(t.word(9)?).and_then(|name|names.id(MESH_TYPE,name)).unwrap_or(0);
        // GC100e4e96 allocates VisualMesh for a resolved name before asynchronous
        // DS1006b623 loading: absent payload leaves a live, nondrawing control.
        let scene=if resource==0 {None} else if let Some(scene)=resources.get(&resource) {Some(Arc::clone(scene))} else {
            let mut scene=Scene::default();
            if let Some(mesh)=ao_formats::mesh::decode_mesh_into(store,resource,&mut scene)? {
                scene.instances.push(ao_scene::Instance {mesh,transform:Mat4::IDENTITY.to_cols_array_2d()});
                let scene=Arc::new(scene);resources.insert(resource,Arc::clone(&scene));Some(scene)
            } else {None}
        };
        let source=super::sprites::connector(t,source)?;
        Ok(Self {t:t.clone(),source,elapsed:0.0,started:false,stopped:resource==0,scene,resource})
    }
    pub(super) fn configure(&mut self,c:EffectConfig) {if let Some(d)=c.duration {{ self.t.words.resize(self.t.words.len().max((8) + 1), 0); *self.t.words.get_mut(8).unwrap() = d.to_bits(); };}}
    pub(super) fn update_source(&mut self,m:Mat4)->Result<()> {self.source=super::sprites::connector(&self.t,m)?;Ok(())}
    pub(super) fn source_removed(&mut self) {if self.t.word(0).unwrap_or(0)&0x200==0 {self.stopped=true;}}
    pub(super) fn needs_ground(&self)->bool {self.t.word(0).unwrap_or(0)&0x100!=0}
    pub(super) fn terminate_gracefully(&mut self) {self.stopped=true;}
    pub(super) fn frame(&mut self,dt:f32)->Result<bool> {ensure!(dt.is_finite()&&dt>=0.0,"invalid Mesh timestep");if self.started {self.elapsed+=dt;}else{self.started=true;}Ok(!self.stopped&&self.elapsed<=self.t.float(8)?)}
    pub(super) fn scene(&self)->Option<(u32,Arc<Scene>)> {self.scene.as_ref().map(|scene|(self.resource,Arc::clone(scene)))}
    pub(super) fn alpha(&self)->f32 {let rise=f32::from_bits(self.t.word(10).unwrap_or(0));let fall=f32::from_bits(self.t.word(11).unwrap_or(0));let end=f32::from_bits(self.t.word(8).unwrap_or(0))-fall;if self.elapsed<rise {self.elapsed/rise}else if self.elapsed>=end {1.0-(self.elapsed-end)/fall}else{1.0}}
    pub(super) fn actor(&self,id:u32,model:u64,ground:&mut dyn FnMut(Vec3)->Option<(Vec3,Vec3)>)->Result<Option<ActorFrame>> {
        if self.scene.is_none() {return Ok(None);}
        let mut p=self.source.w_axis.truncate();if self.needs_ground() {let Some((g,_))=ground(p) else{return Ok(None)};p.y=g.y;}
        p.y+=self.t.float(12)?;
        Ok(Some(ActorFrame {id,model,transform:Mat4::from_rotation_translation(Quat::from_rotation_y(-self.t.float(13)?*self.elapsed),p).to_cols_array_2d(),alpha:self.alpha(),..Default::default()}))
    }
}
fn lerp_argb(a:u32,b:u32,t:f32)->u32 {let a=a.to_be_bytes();let b=b.to_be_bytes();u32::from_be_bytes(std::array::from_fn(|i|(a[i] as f32*(1.0-t)+b[i] as f32*t) as u8))}
fn lerp_color(a:u32,b:u32,t:f32)->[f32;4] {let a=a.to_be_bytes();let b=b.to_be_bytes();super::buff300x::packed_color(u32::from_be_bytes(std::array::from_fn(|i|(a[i] as f32*(1.0-t)+b[i] as f32*t) as u8)))}
fn cone(segments:u32,position:Vec3,rotation:f32,dimensions:[f32;3],colors:[[f32;4];2],uv:[f32;4],swapped:bool)->Vec<Vertex> {
    let [height,bottom,top]=dimensions;
    let mut out=Vec::with_capacity((segments as usize+1)*2);
    for i in 0..=segments {let a=std::f32::consts::TAU*i as f32/segments as f32+rotation;for (end,radius) in [bottom,top].into_iter().enumerate() {let u=i as f32/segments as f32;out.push(Vertex {pos:(position+Vec3::new(a.cos()*radius,end as f32*height,-a.sin()*radius)).to_array(),normal:[0.0,0.0,1.0],uv:if swapped {[end as f32*uv[0]+uv[2],u*uv[1]+uv[3]]}else{[u*uv[0]+uv[2],end as f32*uv[1]+uv[3]]},color:colors[end]});}}
    out
}
fn cone_models(material:u32,count:u32,segments:u32)->Vec<(Option<usize>,Vec<u32>,usize)> {let indices:Vec<_>=(0..segments*2).flat_map(|i|if i&1==0 {[i,i+1,i+2]}else{[i+1,i,i+2]}).collect();(0..count).map(|_|(Some(material as usize),indices.clone(),(segments as usize+1)*2)).collect()}

// Splash3002 loader100f5081, liquid init100f5522, process100f522e.
pub(super) struct Splash {t:Template,position:Vec3,elapsed:f32,started:bool,stopped:bool,liquid:Option<bool>}
impl Splash {
    pub(super) fn new(t:&Template,source:Mat4)->Result<Self> {ensure!(t.kind==3002,"not Splash");for i in [1,2,3,11,12,14,15,16,17,18,19,20,21,24,27,28,29,30,31] {t.float(i)?;}ensure!((1..=4096).contains(&t.word(13)?),"invalid Splash segments");t.word(32)?;let position=source.w_axis.truncate()+Vec3::new(t.float(1)?,t.float(2)?,-t.float(3)?);Ok(Self{t:t.clone(),position,elapsed:0.0,started:false,stopped:false,liquid:None})}
    pub(super) fn configure(&mut self,c:EffectConfig) {if let Some(d)=c.duration {{ self.t.words.resize(self.t.words.len().max((8) + 1), 0); *self.t.words.get_mut(8).unwrap() = d.to_bits(); };}}
    pub(super) fn update_source(&mut self,_:Mat4)->Result<()> {Ok(())}
    pub(super) fn terminate_gracefully(&mut self) {self.stopped=true;}
    pub(super) fn frame(&mut self,dt:f32)->Result<bool> {ensure!(dt.is_finite()&&dt>=0.0,"invalid Splash timestep");if self.started {self.elapsed+=dt;}else{self.started=true;}let t=&self.t;let end=(t.word(10)?.saturating_sub(1)) as f32*t.float(12)?+t.float(11)?*t.float(30)?;Ok(!self.stopped&&self.liquid!=Some(false)&&self.elapsed<end)}
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {cone_models(self.t.word(9).unwrap_or(0),self.t.word(10).unwrap_or(0),self.t.word(13).unwrap_or(0))}
    pub(super) fn blends(&self)->Vec<Blend> {vec![if self.t.word(0).unwrap_or(0)&0x100!=0 {Blend::Additive}else{Blend::AlphaBlend};self.t.word(10).unwrap_or(0) as usize]}
    pub(super) fn vertices(&mut self,liquid:&mut dyn FnMut(Vec3)->Option<(f32,i32)>)->Result<Option<Vec<Vec<Vertex>>>> {
        if self.liquid.is_none() {let Some((height,kind))=liquid(self.position) else{return Ok(None)};self.liquid=Some(kind!=2&&self.position.y<=height);if self.liquid==Some(true) {self.position.y=height;}}
        if self.liquid!=Some(true)||self.stopped {return Ok(None)}
        let t=&self.t;let count=t.word(10)?;let scale=t.float(31)?;let mut out=Vec::with_capacity(count as usize);
        for i in 0..count {let f=i as f32;let age=(self.elapsed-t.float(12)?*f)/t.float(11)?;if age<0.0||age>=t.float(30)? {out.push(vec![Vertex::default();(t.word(13)? as usize+1)*2]);continue}
            let amplitude=scale*(t.float(16)?*(1.0-f/(count-1) as f32)+t.float(17)?*f/(count-1) as f32);
            let parabola=2.0*age-1.0+f*t.float(28)?;let shifted=parabola-t.float(29)?;let bottom_height=if shifted>=-1.0 {(1.0-shifted*shifted)*amplitude}else{0.0};let height=(1.0-parabola*parabola)*amplitude-bottom_height;
            let position=self.position+Vec3::Y*(f*t.float(27)?*scale+bottom_height);
            out.push(cone(t.word(13)?,position,f*t.float(24)?,[height,scale*(t.float(18)?+f*t.float(20)?),scale*(t.float(19)?+f*t.float(21)?)*age],[lerp_color(t.word(22)?,t.word(25)?,age/t.float(30)?),lerp_color(t.word(23)?,t.word(26)?,age/t.float(30)?)],[t.float(14)?,t.float(15)?,0.0,0.0],t.word(0).unwrap_or(0)&0x200!=0));
        }Ok(Some(out))
    }
}

// CrazyCone3009 loader100d6517, init100d6e5a, process100d6874.
pub(super) struct CrazyCone {t:Template,source:Mat4,elapsed:f32,started:bool,stopped:bool,state:usize,state_time:f32,pulse:i32,pulse_time:f32,values:[f32;8],colors:[u32;2]}
impl CrazyCone {
    pub(super) fn new(t:&Template,source:Mat4)->Result<Self> {
        ensure!(t.kind==3009,"not CrazyCone");ensure!((1..=4096).contains(&t.word(11)?),"invalid CrazyCone segments");
        let states=t.word(20)? as usize;ensure!(states>0&&states<=4096,"invalid CrazyCone states");
        for i in 12..=19 {t.float(i)?;}for s in 0..states {for i in 0..17 {t.float(21+s*29+i)?;}for i in 22..27 {t.float(21+s*29+i)?;}ensure!(t.float(21+s*29)?>0.0,"nonpositive CrazyCone state duration");}
        let source=super::sprites::connector(t,source)?;let values=[t.float(14)?,t.float(15)?,t.float(16)?,t.float(17)?,t.float(18)?,t.float(19)?,0.0,0.0];
        Ok(Self{t:t.clone(),source,elapsed:0.0,started:false,stopped:false,state:0,state_time:0.0,pulse:-1,pulse_time:0.0,values,colors:[0;2]})
    }
    pub(super) fn configure(&mut self,_:EffectConfig) {}
    pub(super) fn update_source(&mut self,m:Mat4)->Result<()> {self.source=super::sprites::connector(&self.t,m)?;Ok(())}
    pub(super) fn terminate_gracefully(&mut self) {self.stopped=true;}
    pub(super) fn frame(&mut self,dt:f32)->Result<bool> {
        ensure!(dt.is_finite()&&dt>=0.0,"invalid CrazyCone timestep");let mut left=if self.started {dt}else{self.started=true;0.0};self.elapsed+=left;
        if self.stopped||self.state>=self.t.word(20).unwrap_or(0) as usize {return Ok(false)}
        loop {let base=21+self.state*29;let duration=self.t.float(base)?;let step=left.min(duration-self.state_time);self.state_time+=step;let fraction=self.state_time/duration;
            for (v,a) in [(7,12),(8,13),(5,10),(6,11),(3,14),(4,15),(9,16)] {let value=self.t.float(base+v)?+step*self.t.float(base+a)?;{ self.t.words.resize(self.t.words.len().max((base+v) + 1), 0); *self.t.words.get_mut(base+v).unwrap() = value.to_bits(); };}
            for (value,velocity) in [(0,1),(1,2),(2,7),(3,8),(4,5),(5,6),(6,3),(7,4)] {self.values[value]+=step*self.t.float(base+velocity)?;}
            self.colors=[lerp_argb(self.t.word(base+18).unwrap_or(0),self.t.word(base+20).unwrap_or(0),fraction),lerp_argb(self.t.word(base+17).unwrap_or(0),self.t.word(base+19).unwrap_or(0),fraction)];
            let pulses=self.t.word(base+21).unwrap_or(0) as i32;
            if pulses!=0 && (self.pulse>=0||self.state_time>self.t.float(base+22)?) {
                if self.pulse<0 {self.pulse=0;self.pulse_time=0.0;}self.pulse_time+=step;
                let attack=self.t.float(base+25)?;let release=self.t.float(base+24)?;
                let factor=if self.pulse_time<attack {self.pulse_time/attack}else if self.pulse_time<release {1.0-(self.pulse_time-attack)/(release-attack)}else{0.0};
                for (end,word) in [(0,28),(1,27)] {self.colors[end]=lerp_argb(self.colors[end],self.t.word(base+word).unwrap_or(0),factor);}
                if self.pulse_time>release+self.t.float(base+23)? {self.pulse+=1;self.pulse_time=0.0;if self.pulse==pulses-1 {{ self.t.words.resize(self.t.words.len().max((base+24) + 1), 0); *self.t.words.get_mut(base+24).unwrap() = (release+self.t.float(base+26)?).to_bits(); };}else if self.pulse==pulses {{ self.t.words.resize(self.t.words.len().max((base+21) + 1), 0); *self.t.words.get_mut(base+21).unwrap() = 0; };}}
            }
            left-=step;if self.state_time<duration||left<=0.0 {break}self.state+=1;self.state_time=0.0;self.pulse= -1;if self.state>=self.t.word(20).unwrap_or(0) as usize {break}
        }Ok(true)
    }
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {cone_models(self.t.word(9).unwrap_or(0),self.t.word(10).unwrap_or(0),self.t.word(11).unwrap_or(0))}
    pub(super) fn blends(&self)->Vec<Blend> {vec![if self.t.word(0).unwrap_or(0)&0x200!=0 {Blend::Additive}else{Blend::AlphaBlend};self.t.word(10).unwrap_or(0) as usize]}
    pub(super) fn vertices(&self,ground:&mut dyn FnMut(Vec3)->Option<(Vec3,Vec3)>)->Result<Option<Vec<Vec<Vertex>>>> {
        let mut position=self.source.w_axis.truncate();if self.t.word(0).unwrap_or(0)&0x800!=0 {let Some((g,_))=ground(position) else{return Ok(None)};position.y=g.y;}position.y+=self.values[5];
        let base=21+self.state.min(self.t.word(20).unwrap_or(0) as usize-1)*29;let count=self.t.word(10).unwrap_or(0);
        Ok(Some((0..count).map(|i|cone(self.t.word(11).unwrap_or(0),position,self.state_time*f32::from_bits(self.t.word(base+9).unwrap_or(0))+std::f32::consts::TAU*i as f32/count as f32,[self.values[4]-self.values[5],self.values[3]+i as f32*f32::from_bits(self.t.word(13).unwrap_or(0)),self.values[2]+i as f32*f32::from_bits(self.t.word(12).unwrap_or(0))],self.colors.map(super::buff300x::packed_color),[self.values[0],self.values[1],self.values[6],self.values[7]],self.t.word(0).unwrap_or(0)&0x100!=0)).collect()))
    }
}

// WaterRipples3005 loader10103c1c/process101042de/emission10104162;
// DS100307b0/1003049f generates expanding annular strips.
struct Ring {position:Vec3,rotation:Quat,born:f32}
pub(super) struct WaterRipples {t:Template,source:Mat4,previous:Vec3,elapsed:f32,last:f32,started:bool,stopped:bool,rings:Vec<Ring>,depth:f32,liquid_flags:u32,direction:Vec3}
impl WaterRipples {
    pub(super) fn new(t:&Template,source:Mat4)->Result<Self> {ensure!(t.kind==3005,"not WaterRipples");t.word(24)?;ensure!((1..=4096).contains(&t.word(5)?),"invalid WaterRipples segments");for i in [2,3,4,8,10,11,15,16,20,21,22,23,24] {t.float(i)?;}ensure!(t.float(2)?>0.0,"invalid WaterRipples lifetime");Ok(Self{t:t.clone(),source,previous:source.w_axis.truncate(),elapsed:0.0,last:-1.0e30,started:false,stopped:false,rings:Vec::new(),depth:0.0,liquid_flags:0,direction:Vec3::Y})}
    pub(super) fn configure(&mut self,c:EffectConfig) {if let Some(d)=c.duration {{ self.t.words.resize(self.t.words.len().max((8) + 1), 0); *self.t.words.get_mut(8).unwrap() = d.to_bits(); };}}
    pub(super) fn update_source(&mut self,m:Mat4)->Result<()> {self.source=m;Ok(())}
    pub(super) fn update_liquid(&mut self,depth:f32,flags:u32,direction:Vec3) {self.depth=depth;self.liquid_flags=flags;self.direction=direction;}
    pub(super) fn clear_liquid(&mut self) {self.depth= -9999.0;}
    pub(super) fn terminate_gracefully(&mut self) {self.stopped=true;}
    pub(super) fn frame(&mut self,dt:f32)->Result<bool> {
        ensure!(dt.is_finite()&&dt>=0.0,"invalid WaterRipples timestep");
        if self.started {self.elapsed+=dt;}else{self.started=true;}
        if self.t.float(8)?>0.0&&self.elapsed>self.t.float(8)? {self.stopped=true;}
        if !self.stopped&&self.depth>0.01&&self.liquid_flags&0x1e!=2&&dt>0.0 {
            let p=self.source.w_axis.truncate()+Vec3::Y*self.depth;let speed=p.distance(self.previous)/dt;self.previous=p;
            if self.elapsed-self.last>self.t.float(22)?.min(self.t.float(23)?+self.t.float(24)?*speed) {
                let direction=self.direction.try_normalize().context("invalid ripple liquid normal")?;
                let z=(-direction.cross(Vec3::X)).try_normalize().context("invalid ripple liquid orientation")?;let x=direction.cross(z);let rotation=Quat::from_mat3(&glam::Mat3::from_cols(x,direction,z));
                // GC10104162 applies word4 only when VisualWater global102e3468 is nonzero; all native writers set it to zero.
                self.rings.push(Ring {position:p,rotation,born:self.elapsed});self.last=self.elapsed;
            }
        }
        let lifetime=self.t.float(2)?;self.rings.retain(|r|self.elapsed-r.born<=lifetime);Ok(!self.stopped||!self.rings.is_empty())
    }
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {cone_models(self.t.word(9).unwrap_or(0),self.rings.len() as u32,self.t.word(5).unwrap_or(0))}
    pub(super) fn blends(&self)->Vec<Blend> {vec![Blend::AlphaBlend;self.rings.len()]}
    pub(super) fn vertices(&self)->Result<Vec<Vec<Vertex>>> {
        let t=&self.t;let n=t.word(5)?;let lifetime=t.float(2)?;let mut out=Vec::with_capacity(self.rings.len());
        for ring in &self.rings {let fraction=(self.elapsed-ring.born)/lifetime;let inner=(t.float(10)?+(t.float(15)?-t.float(10)?)*fraction).max(0.0);let outer=(t.float(11)?+(t.float(16)?-t.float(11)?)*fraction).max(inner);let alpha=((1.0-fraction)*64.0) as u32;
            let mut vertices=Vec::with_capacity((n as usize+1)*2);for i in 0..=n {let angle=std::f32::consts::TAU*i as f32/n as f32;for (end,radius) in [inner,outer].into_iter().enumerate() {vertices.push(Vertex {pos:(ring.position+ring.rotation*Vec3::new(angle.cos()*radius,0.0,angle.sin()*radius)).to_array(),normal:[0.0,1.0,0.0],uv:[i as f32*t.float(20)?/n as f32,end as f32*t.float(21)?],color:super::buff300x::packed_color((alpha<<24)|0x7e7e7e)});}}out.push(vertices);
        }Ok(out)
    }
}

// Shadow3008 GC100ece3a/100ecee4/100ecb6b; DS1001e75d/1001e90e.
pub(super) struct Shadow {vertices:Vec<Vertex>,origin:Vec3,projection:Option<(Vec3,Vec3,Vec3,Vec3,bool,bool)>,stopped:bool,enabled:bool}
impl Shadow {
    pub(super) fn new(t:&Template,_source:Mat4)->Result<Self> {ensure!(t.kind==3008,"not Shadow");Ok(Self {vertices:Vec::new(),origin:Vec3::ZERO,projection:None,stopped:false,enabled:true})}
    pub(super) fn configure(&mut self,_:EffectConfig) {}
    pub(super) fn update_source(&mut self,_:Mat4)->Result<()> {Ok(())}
    pub(super) fn terminate_gracefully(&mut self) {self.stopped=true;}
    pub(super) fn frame(&mut self,dt:f32)->Result<bool> {ensure!(dt.is_finite()&&dt>=0.0,"invalid Shadow timestep");Ok(!self.stopped)}
    pub(super) fn set_enabled(&mut self,enabled:bool) {self.enabled=enabled;}
    pub(super) fn update_projection(&mut self,direction:Vec3,camera:Vec3,point:Vec3,normal:Vec3,dungeon:bool,warp:bool)->Result<()> {ensure!(direction.is_finite()&&camera.is_finite()&&point.is_finite()&&normal.is_finite(),"invalid Shadow projection");self.projection=Some((direction,camera,point,normal,dungeon,warp));Ok(())}
    pub(super) fn update_world(&mut self,body_head:(Vec3,f32),sun:Vec3,camera:Vec3,dungeon:bool,ground:&mut dyn FnMut(Vec3)->Option<(Vec3,Vec3)>,ray:&mut dyn FnMut(Vec3,Vec3)->Option<(Vec3,Vec3)>)->Result<()> {
        let (body,head_height)=body_head;
        ensure!(head_height.is_finite()&&body.is_finite()&&sun.is_finite()&&camera.is_finite(),"invalid Shadow world inputs");
        self.projection=None;let Some((floor,floor_normal))=ground(body) else{return Ok(())};
        if dungeon {return self.update_projection(-Vec3::Y,camera,floor,floor_normal,true,false)}
        let height=if head_height<0.1 {2.0}else{head_height};if floor.y+0.2>=body.y+height {return Ok(())}
        let direction=if sun.length_squared()<=0.001 {-Vec3::Y}else{sun.normalize()};
        let top=body+Vec3::Y*height;let half=body+Vec3::Y*(height*0.5);
        let upper=ray(top,top+direction*20.0);let upper=match upper {Some((p,n)) if n.y>=0.5=>Some((p,n)),_=>ray(half,half+direction*20.0)};
        let Some((mut upper,upper_normal))=upper else{return Ok(())};
        let (mut lower,lower_normal)=if floor.y+0.5<=body.y {let Some(hit)=ray(body-direction,body+direction*20.0) else{return Ok(())};hit}else{(body,-Vec3::Y)};
        let mut warp=true;
        for (point,normal) in [(&mut upper,upper_normal),(&mut lower,lower_normal)] {if let Some((terrain,_))=ground(*point) {if normal.y<0.5 {point.y=terrain.y;}else if (terrain.y-point.y).abs()>0.2 {warp=false;}}}
        let tangent=upper-lower;if tangent.length_squared()<=0.0001 {return Ok(())}let tangent=tangent.normalize();let normal=tangent.cross(Vec3::Y).cross(tangent);
        self.update_projection(direction,camera,lower,normal,false,warp)
    }
    pub(super) fn prepare_source(&mut self,scene:&Scene,actor:&ActorFrame)->Result<bool> {
        self.vertices.clear();let transform=Mat4::from_cols_array_2d(&actor.transform);self.origin=transform.w_axis.truncate();
        for (index,mesh) in scene.meshes.iter().enumerate() {let pose=if index==0 {actor.skin.as_deref().unwrap_or(&mesh.vertices)}else{&mesh.vertices};let part=actor.parts.get(index).map(Mat4::from_cols_array_2d).unwrap_or(Mat4::IDENTITY);let world=transform*part;self.vertices.extend(pose.iter().step_by(4).map(|v|{let mut v=*v;v.pos=world.transform_point3(Vec3::from(v.pos)).to_array();v}));}
        Ok(false)
    }
    pub(super) fn geometry(&self,ground:&mut dyn FnMut(Vec3)->Option<(Vec3,Vec3)>)->Result<Option<(Vec<Vertex>,Vec<u32>)>> {
        if !self.enabled||self.stopped||self.vertices.is_empty() {return Ok(None)}
        let Some((direction,camera,point,normal,dungeon,warp))=self.projection else{return Ok(None)};
        let dot=direction.dot(normal);if dot>=-0.1 {return Ok(None)}
        let (axis_a,axis_b)=if direction.y.abs()>=0.95 {let b=direction.cross(Vec3::X);(b.cross(direction),b)}else{let a=Vec3::Y.cross(direction);(a,direction.cross(a))};
        let mut low=glam::Vec2::splat(f32::INFINITY);let mut high=glam::Vec2::splat(f32::NEG_INFINITY);
        for v in &self.vertices {let p=Vec3::from(v.pos);if p.y>=point.y {let p=p-self.origin;let q=glam::Vec2::new(p.dot(axis_a),p.dot(axis_b));low=low.min(q);high=high.max(q);}}
        let radius=(high-low)*0.5;if radius.min_element()<=0.0||!radius.is_finite() {return Ok(None)}
        let size=(radius.length().sqrt()+0.2)*(1.0-self.origin.distance(camera)/50.0);let size=size.max(0.0);
        let exponent=(size*20.0+1.0).log2().floor() as u32;let segments=(1u32.checked_shl(exponent).unwrap_or(300)).min(300);if segments<4 {return Ok(None)}
        let center=self.origin+axis_a*((high.x+low.x)*0.5)+axis_b*((high.y+low.y)*0.5);
        let project=|p:Vec3| {let distance=(p.dot(normal)-point.dot(normal))*(-1.0/dot);(p+direction*distance,distance)};
        let alpha=(((if dungeon {47.0}else{95.0})*((-dot-0.1)/0.3).min(1.0)).floor() as u32)<<24;
        let mut vertices=Vec::with_capacity((segments*2+1) as usize);let (projected_center,_)=project(center);vertices.push(Vertex {pos:projected_center.to_array(),normal:normal.to_array(),uv:[0.0;2],color:super::buff300x::packed_color(alpha)});
        for i in 0..segments {let angle=std::f32::consts::TAU*i as f32/segments as f32;let offset=axis_a*(angle.cos()*radius.x)+axis_b*(angle.sin()*radius.y);let inner_scale=dot*(-0.4)+1.0;let (inner,distance)=project(center+offset*inner_scale);let outer_scale=0.7-distance*(-0.2)*dot;let outer_scale=if outer_scale<0.1 {0.1}else{outer_scale};let (outer,_)=project(center+offset*outer_scale);for (p,color) in [(outer,alpha),(inner,0)] {vertices.push(Vertex {pos:p.to_array(),normal:normal.to_array(),uv:[0.0;2],color:super::buff300x::packed_color(color)});}}
        if warp&&!dungeon {for v in &mut vertices {let p=Vec3::from(v.pos);if let Some((g,_))=ground(p) {v.pos[1]=g.y+0.02;}}}
        let mut indices=Vec::with_capacity(segments as usize*9);for i in 0..segments {let a=1+i*2;let b=1+((i+1)%segments)*2;indices.extend([0,a,b,a,a+1,b,b,a+1,b+1]);}
        Ok(Some((vertices,indices)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn native_mesh_resource_names() {assert_eq!(tower_name(0),Some("tower_destroyed_buff&debuff.abiff"));assert_eq!(tower_name(3),Some("tower_destroyed_guard.abiff"));assert_eq!(tower_name(4),None);}
    #[test] fn native_mesh_alpha_envelope() {let mut words=vec![0;14];words[8]=4f32.to_bits();words[10]=1f32.to_bits();words[11]=2f32.to_bits();let mut mesh=TowerMesh {t:Template {kind:3010,words},source:Mat4::IDENTITY,elapsed:0.5,started:true,stopped:false,scene:Some(Arc::new(Scene::default())),resource:0};assert_eq!(mesh.alpha(),0.5);mesh.elapsed=2.0;assert_eq!(mesh.alpha(),1.0);mesh.elapsed=3.0;assert_eq!(mesh.alpha(),0.5);}
    #[test]
    fn native_tower_source_terrain_and_lifecycle() {
        let mut words=vec![0;14];words[0]=0x100;words[8]=4f32.to_bits();words[10]=1f32.to_bits();words[11]=1f32.to_bits();words[12]=2f32.to_bits();words[13]=0.5f32.to_bits();
        let mut mesh=TowerMesh {t:Template {kind:3010,words},source:Mat4::from_translation(Vec3::new(3.0,7.0,5.0)),elapsed:0.0,started:false,stopped:false,scene:Some(Arc::new(Scene::default())),resource:9};
        assert!(mesh.frame(1.0).unwrap());assert_eq!(mesh.elapsed,0.0);assert!(mesh.frame(1.0).unwrap());
        assert!(mesh.needs_ground());assert!(mesh.actor(1,9,&mut |_|None).unwrap().is_none());
        let actor=mesh.actor(1,9,&mut |p|Some((Vec3::new(p.x,11.0,p.z),Vec3::Y))).unwrap().unwrap();
        let transform=Mat4::from_cols_array_2d(&actor.transform);assert_eq!(transform.w_axis.truncate(),Vec3::new(3.0,13.0,5.0));assert_eq!(actor.alpha,1.0);
        *mesh.t.words.get_mut(0).unwrap() |= 0x200;mesh.source_removed();assert!(mesh.frame(0.0).unwrap());
        *mesh.t.words.get_mut(0).unwrap() &= !0x200;mesh.source_removed();assert!(!mesh.frame(0.0).unwrap());
        mesh.stopped=false;mesh.terminate_gracefully();assert!(!mesh.frame(0.0).unwrap());
    }
    #[test]
    fn authored_legacy300x_records() {
        let dir=ao_gui::client_dir();let templates=super::super::Templates::open(&dir).unwrap();let store=RecordStore::open(&dir).unwrap();let names=NameTable::load(&store).unwrap();
        let mut counts=[0;5];let source=Mat4::IDENTITY;
        for (&id,t) in &templates.by_id {match t.kind {
            3002=>{counts[0]+=1;let mut e=Splash::new(t,source).unwrap();e.frame(0.0).unwrap();let _=e.vertices(&mut |_|Some((0.0,0))).unwrap();assert_eq!(e.models().len(),t.words[10] as usize,"{id}");},
            3005=>{counts[1]+=1;let mut e=WaterRipples::new(t,source).unwrap();assert_eq!(e.source,source,"WaterRipples does not use generic connector fields");e.update_liquid(1.0,0,Vec3::Y);e.frame(1.0/60.0).unwrap();assert_eq!(e.rings[0].position,Vec3::Y);assert_eq!(e.rings[0].rotation,Quat::IDENTITY);assert!(!e.vertices().unwrap().is_empty(),"{id}");},
            3008=>{counts[2]+=1;assert!(Shadow::new(t,source).unwrap().frame(0.0).unwrap());},
            3009=>{counts[3]+=1;let mut e=CrazyCone::new(t,source).unwrap();e.frame(0.0).unwrap();let v=e.vertices(&mut |p|Some((Vec3::new(p.x,0.0,p.z),Vec3::Y))).unwrap().unwrap();assert_eq!(v.len(),t.words[10] as usize,"{id}");assert!(v.iter().flatten().all(|v|v.pos.iter().all(|x|x.is_finite())),"{id}");},
            3010=>{counts[4]+=1;let mut resources=std::collections::HashMap::new();let mut e=TowerMesh::new(t,&store,&names,source,&mut resources).unwrap();let again=TowerMesh::new(t,&store,&names,source,&mut resources).unwrap();if id==61042 {assert_eq!(t.word(9).unwrap(),1);assert_eq!(e.resource,201713);assert_eq!(names.name(MESH_TYPE,e.resource),Some("tower_destroyed_buff&debuff_LL.abiff"));assert!(store.get(MESH_TYPE,e.resource).unwrap().is_none(),"installed name is present but mesh payload absent");assert!(e.scene().is_none());assert!(e.actor(1,1,&mut |_|None).unwrap().is_none());assert!(e.frame(0.0).unwrap(),"resolved native VisualMesh retains its timer");assert!(!e.frame(t.float(8).unwrap()+1.0).unwrap());}else{let scene=e.scene.as_ref().expect("other authored tower payload");assert!(Arc::ptr_eq(scene,again.scene.as_ref().unwrap()));assert!(!scene.meshes.is_empty(),"{id}");}},
            _=>{}
        }}
        assert_eq!(counts,[6,1,2,11,5]);
    }
    #[test]
    #[ignore="retail meshes and offscreen Metal renderer"]
    fn authored_legacy300x_tower_frames() {
        let dir=ao_gui::client_dir();let templates=super::super::Templates::open(&dir).unwrap();let store=RecordStore::open(&dir).unwrap();let names=NameTable::load(&store).unwrap();
        let out=std::path::PathBuf::from(std::env::var_os("AOMAC_EFFECT_FRAMES").expect("AOMAC_EFFECT_FRAMES output directory"));std::fs::create_dir_all(&out).unwrap();
        for (&id,t) in templates.by_id.iter().filter(|(_,t)|t.kind==3010) {
            let mut e=TowerMesh::new(t,&store,&names,Mat4::IDENTITY,&mut std::collections::HashMap::new()).unwrap();e.frame(0.0).unwrap();
            let Some((_,scene))=e.scene() else {assert_eq!(id,61042,"only proven installed tower payload hole");assert_eq!(e.resource,201713);continue;};
            for frame in 1..=120 {
                if !e.frame(1.0/60.0).unwrap() {break}
                if ![15,30,60,120].contains(&frame) {continue}
                let actor=e.actor(1,1,&mut |p|Some((Vec3::new(p.x,0.0,p.z),Vec3::Y))).unwrap().unwrap();let transform=Mat4::from_cols_array_2d(&actor.transform);
                let mut low=Vec3::splat(f32::INFINITY);let mut high=Vec3::splat(f32::NEG_INFINITY);for mesh in &scene.meshes {for v in &mesh.vertices {let p=transform.transform_point3(Vec3::from(v.pos));low=low.min(p);high=high.max(p);}}
                let center=(low+high)*0.5;let radius=((high-low).length()*0.5).max(0.5);let eye=center+Vec3::new(0.6,0.5,1.0).normalize()*radius*2.4;
                let path=out.join(format!("legacy3010_{id}_{frame}.png"));ao_render::render_to_png_actors(&Scene::default(),&[(1,scene.as_ref().clone())],vec![actor],eye.to_array(),center.to_array(),640,480,&path,frame as f32/60.0).unwrap();
                assert!(path.is_file());
            }
        }
    }
    #[test]
    fn native_shadow_ray_selection_and_dungeon_projection() {
        let mut shadow=Shadow::new(&Template {kind:3008,words:Vec::new()},Mat4::IDENTITY).unwrap();
        let body=Vec3::new(0.0,1.0,0.0);let mut calls=Vec::new();
        shadow.update_world((body,2.0),Vec3::new(1.0,-1.0,0.0),Vec3::new(0.0,2.0,4.0),false,&mut |p|Some((Vec3::new(p.x,0.0,p.z),Vec3::Y)),&mut |from,to| {calls.push((from,to));let f=from.y/(from.y-to.y);Some((from+(to-from)*f,Vec3::Y))}).unwrap();
        assert_eq!(calls.len(),2);assert_eq!(calls[0].0,body+Vec3::Y*2.0);assert!((calls[1].0-(body-Vec3::new(1.0,-1.0,0.0).normalize())).length()<1e-6);assert!(shadow.projection.is_some());
        shadow.update_world((body,2.0),Vec3::ZERO,Vec3::ZERO,true,&mut |p|Some((Vec3::new(p.x,0.0,p.z),Vec3::Y)),&mut |_,_|panic!("dungeon shadows must not sun-raycast")).unwrap();
        let (direction,_,_,normal,dungeon,warp)=shadow.projection.unwrap();assert_eq!(direction,-Vec3::Y);assert_eq!(normal,Vec3::Y);assert!(dungeon&&!warp);
    }
    fn capture_native_geometry(resources:(&RecordStore,&NameTable),id:i32,frame:u32,parts:Vec<Vec<Vertex>>,models:Vec<(Option<usize>,Vec<u32>,usize)>,blends:Vec<Blend>,out:&std::path::Path) {
        let (store,names)=resources;
        let mut scene=Scene::default();let mut vertices=Vec::new();let mut submeshes=Vec::new();
        for ((part,(material,indices,_)),blend) in parts.into_iter().zip(models).zip(blends) {
            let key=material.map(|m| {let name=super::super::materials::MATERIALS[m].0;ao_scene::TextureKey {rdb_type:1010004,id:names.id(1010004,name).unwrap()}});
            if let Some(key)=key {if let std::collections::hash_map::Entry::Vacant(entry)=scene.textures.entry(key) {entry.insert(ao_formats::texture::load_texture(store,key).unwrap().unwrap());}}
            let offset=vertices.len() as u32;let mut sub=ao_scene::Submesh::new(indices.into_iter().map(|i|i+offset).collect(),key);sub.blend=blend;sub.two_sided=true;sub.emissive=[1.0;3];submeshes.push(sub);vertices.extend(part);
        }
        assert!(vertices.iter().all(|v|v.pos.iter().all(|p|p.is_finite())),"{id}/{frame}");
        let mut low=Vec3::splat(f32::INFINITY);let mut high=Vec3::splat(f32::NEG_INFINITY);for v in &vertices {let p=Vec3::from(v.pos);low=low.min(p);high=high.max(p);}
        assert!(!vertices.is_empty());let center=(low+high)*0.5;let radius=((high-low).length()*0.5).max(0.5);let eye=center+Vec3::new(0.6,0.5,1.0).normalize()*radius*2.4;
        scene.meshes.push(ao_scene::Mesh {vertices,submeshes});scene.instances.push(ao_scene::Instance {mesh:0,transform:Mat4::IDENTITY.to_cols_array_2d()});
        let path=out.join(format!("legacy_{id}_{frame}.png"));ao_render::render_to_png_at(&scene,eye.to_array(),center.to_array(),640,480,&path,frame as f32/60.0).unwrap();assert!(path.is_file());
        let blank=out.join(format!("legacy_{id}_{frame}_background.png"));ao_render::render_to_png_at(&Scene::default(),eye.to_array(),center.to_array(),640,480,&blank,frame as f32/60.0).unwrap();
        let pixels=image::open(&path).unwrap().to_rgba8();let background=image::open(&blank).unwrap().to_rgba8();std::fs::remove_file(blank).unwrap();assert!(pixels.pixels().zip(background.pixels()).any(|(a,b)|a!=b),"native effect {id}/{frame} produced no pixels");
    }
    #[test]
    #[ignore="authored retail materials and offscreen Metal renderer"]
    fn authored_legacy300x_surface_frames() {
        let dir=ao_gui::client_dir();let templates=super::super::Templates::open(&dir).unwrap();let store=RecordStore::open(&dir).unwrap();let names=NameTable::load(&store).unwrap();
        let out=std::path::PathBuf::from(std::env::var_os("AOMAC_EFFECT_FRAMES").expect("AOMAC_EFFECT_FRAMES output directory"));std::fs::create_dir_all(&out).unwrap();
        for (&id,t) in templates.by_id.iter().filter(|(_,t)|matches!(t.kind,3002|3005|3008|3009)) {
            let source=Mat4::IDENTITY;let mut floor=|p:Vec3|Some((Vec3::new(p.x,0.0,p.z),Vec3::Y));
            match t.kind {
                3002=>{let mut e=Splash::new(t,source).unwrap();e.frame(0.0).unwrap();for frame in 1..=30 {e.frame(1.0/60.0).unwrap();if [3,6,15,30].contains(&frame) {let parts=e.vertices(&mut |_|Some((0.0,0))).unwrap().unwrap();capture_native_geometry((&store,&names),id,frame,parts,e.models(),e.blends(),&out);}}},
                3005=>{let mut e=WaterRipples::new(t,source).unwrap();e.update_liquid(1.0,0,Vec3::Y);e.frame(0.0).unwrap();for frame in 1..=60 {e.frame(1.0/60.0).unwrap();if [3,15,30,60].contains(&frame) {capture_native_geometry((&store,&names),id,frame,e.vertices().unwrap(),e.models(),e.blends(),&out);}}},
                3009=>{let mut e=CrazyCone::new(t,source).unwrap();e.frame(0.0).unwrap();for frame in 1..=60 {
                    if !e.frame(1.0/60.0).unwrap() {break}
                    if [3,15,30,60].contains(&frame) {capture_native_geometry((&store,&names),id,frame,e.vertices(&mut floor).unwrap().unwrap(),e.models(),e.blends(),&out);}
                }},
                3008=>{let resource=names.id(MESH_TYPE,tower_name(2).unwrap()).unwrap();let scene=load_mesh(&store,resource).unwrap();let mut e=Shadow::new(t,source).unwrap();e.prepare_source(&scene,&ActorFrame {transform:Mat4::from_translation(Vec3::Y).to_cols_array_2d(),..Default::default()}).unwrap();for (frame,dungeon) in [(0,false),(1,true)] {e.update_world((Vec3::Y,2.0),Vec3::new(1.0,-1.0,0.0),Vec3::new(0.0,2.0,4.0),dungeon,&mut floor,&mut |from,to| {let f=from.y/(from.y-to.y);Some((from+(to-from)*f,Vec3::Y))}).unwrap();let (vertices,indices)=e.geometry(&mut floor).unwrap().unwrap();let count=vertices.len();capture_native_geometry((&store,&names),id,frame,vec![vertices],vec![(None,indices,count)],vec![Blend::AlphaBlend],&out);}},
                _=>unreachable!()
            }
        }
    }
}

#[cfg(test)]
#[test]
fn short_splash_defaults_missing_fields_to_zero() {
    let mut words=vec![0;14];words[13]=3;
    let mut effect=Splash::new(&Template {kind:3002,words},Mat4::IDENTITY).unwrap();
    assert_eq!(effect.t.float(31).unwrap(),0.0);
    assert_eq!(effect.models().len(),0);
    assert!(effect.blends().is_empty());
    assert!(!effect.frame(0.0).unwrap());
}

#[cfg(test)]
#[test]
fn short_mesh_configuration_materializes_only_zero_defaults() {
    let mut mesh=TowerMesh {t:Template {kind:3010,words:Vec::new()},source:Mat4::IDENTITY,elapsed:0.0,started:false,stopped:false,scene:None,resource:0};
    mesh.configure(EffectConfig {duration:Some(2.0),..Default::default()});
    assert_eq!(mesh.t.float(8).unwrap(),2.0);
    assert_eq!(mesh.t.word(0).unwrap(),0);
    assert!(!mesh.needs_ground());
    assert_eq!(mesh.alpha(),1.0);
    mesh.source_removed();
    assert!(!mesh.frame(0.0).unwrap());
}
