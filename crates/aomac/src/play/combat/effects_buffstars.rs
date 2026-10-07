//! Native class 2004: GC parameter decoder 100f6d54; Stars initialization
//! 100f7a63 and Process 100f7f3c; DS GfxVisualDiaBill 1001105e.
use super::{buff200x::{authored_color, circle}, materials, quad, sprites, EffectConfig, Template};
use anyhow::{ensure, Result};
use ao_formats::{character::CrtRand, weather::R250};
use ao_scene::{Blend, Vertex};
use glam::{Mat4, Vec3};

/// CMSGet integer/float accessors return zero for absent words. Duration at 26
/// deliberately replaces the common lifetime at 8 (100f6d54 + 0x69).
#[derive(Clone, Debug)]
pub(super) struct StarsParameters {
    pub material: u32,
    pub mode: u32,
    pub shape: [f32; 7],
    pub colors: [[f32; 4]; 2],
    pub duration: f32,
    pub offset: f32,
    pub size: f32,
    pub radius: f32,
    pub lifetime_ms: u32,
    pub count: u32,
}
impl StarsParameters {
    pub(super) fn decode(t: &Template) -> Result<Self> {
        ensure!(t.kind == 2004, "Stars requires native class 2004");
        let word = |i: usize| t.words.get(i).copied().unwrap_or(0);
        let float = |i| f32::from_bits(word(i));
        let mode = word(10);
        ensure!(mode <= 28, "unsupported native Stars mode {mode}");
        let p = Self {
            material: word(9), mode,
            shape: std::array::from_fn(|i| float(11 + i)),
            colors: std::array::from_fn(|k| std::array::from_fn(|i| float(18 + k * 4 + i))),
            duration: float(26), offset: float(27), size: float(28), radius: float(29),
            lifetime_ms: word(30), count: word(31),
        };
        ensure!(p.shape.iter().chain(p.colors.iter().flatten()).chain([&p.duration, &p.offset, &p.size, &p.radius]).all(|v| v.is_finite()), "non-finite native Stars parameter");
        Ok(p)
    }
}

const FRAMES: [f32;16] = [8.,8.,8.,8.,8.,12.,16.,19.,22.,23.,24.,25.,26.,27.,28.,29.];
const EXPLOSION: [f32;16] = [10.,6.,7.,7.,9.,11.,16.,19.,22.,23.,24.,25.,26.,27.,28.,29.];
const PALETTE: [u32;8] = [0xffffc0c0,0xffffff00,0xffff00ff,0xff00ffff,0xff0000ff,0xff00ff00,0xffff0000,0xffc0ffc0];
const BONES: [i32;12] = [1013,1011,1014,1012,1000,1006,1009,1007,1010,1008,1007,1008];
fn mirror(v:Vec3)->Vec3 {v*Vec3::new(1.,1.,-1.)}
fn perpendicular(v:Vec3)->Vec3 {
    // GC100d2e0e; same native formula as effects_beams.
    let mut p=Vec3::new(v.y-v.z,v.z-v.x,v.x-v.y);
    if p==Vec3::ZERO {p=Vec3::new(v.z+v.y,v.z-v.x,-v.x-v.y);}
    p.normalize()
}
fn inside(r:&mut CrtRand,sphere:bool)->Vec3 {
    loop {
        let x=r.rand() as f32*(2./32768.)-1.;
        let y=if sphere {r.rand() as f32*(2./32768.)-1.}else{0.};
        let z=r.rand() as f32*(2./32768.)-1.;
        let v=Vec3::new(x,y,z);
        if v.length_squared()<1. {return v;}
    }
}
fn unpack(c:u32)->[f32;4] {let [a,r,g,b]=c.to_be_bytes();[r,g,b,a].map(|v|v as f32/255.)}
fn packed(c:[f32;4])->u32 {
    let [r,g,b,a]=c.map(|v|((v as f64*255.) as i32 as u32)&255);
    a<<24|r<<16|g<<8|b
}
fn render_color(c:[f32;4])->[f32;4] {
    let mut color=unpack(packed(c));
    for v in &mut color[..3] {*v=v.powf(2.2);}
    color
}
fn hump(u:f32)->f32 {1.-(1.-2.*u).powi(2)}
#[derive(Clone,Copy)]
struct Star {position:Vec3,width:f32,height:f32,color:[f32;4],frame:i32,active:bool}
impl Default for Star {fn default()->Self {Self{position:Vec3::ZERO,width:0.,height:0.,color:[1.;4],frame:0,active:false}}}
pub(super) struct StarsEffect {
    template:Template,p:StarsParameters,source:Mat4,target:Mat4,body_root:Option<Mat4>,
    stars:[Star;128],a:[Vec3;128],b:[Vec3;128],c:[Vec3;128],expiry:[f32;128],
    chain:[Option<Vec3>;12],sampled:[Vec3;30],normals:[Vec3;30],body_ready:bool,
    sample_index:usize,phase:f32,head:usize,direction:Vec3,axis:Vec3,
    stop:bool,alive:bool,ticks:u32,ground:Option<f32>,
}
impl StarsEffect {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(t:&Template,source:Mat4,target:Mat4,_color:u32,_gc:&mut R250,_ds:&mut R250,crt:&mut CrtRand)->Result<Self> {
        let mut p=StarsParameters::decode(t)?;
        ensure!((p.material as usize)<materials::MATERIALS.len(),"unknown Stars material");
        if p.mode==2 {p.duration=1.5;}
        // GC100f6d54 stores word 0 at +0x1670 only to forward it to
        // InitDynelTemplate/InitPositionTemplate (100f751f/100f6f58).
        // Keep those common connector flags in the template: connector applies
        // the native bit-4 rotation order on creation and every source update.
        let mut s=Self {template:t.clone(),p,source:sprites::connector(t,source)?,target,body_root:None,
            stars:[Star::default();128],a:[Vec3::ZERO;128],b:[Vec3::ZERO;128],c:[Vec3::ZERO;128],expiry:[-100.;128],
            chain:[None;12],sampled:[Vec3::ZERO;30],normals:[Vec3::Y;30],body_ready:false,sample_index:0,phase:0.,head:128,direction:Vec3::Y,axis:Vec3::Z,
            stop:false,alive:true,ticks:0,ground:None};
        match s.p.mode {
            0|2=>for i in 0..128 {s.a[i]=if s.p.mode==0 {sprites::sphere_point(crt)}else{inside(crt,true)};s.stars[i].active=true;s.stars[i].width=1.;s.stars[i].height=1.;},
            1|21|22|23=>s.expiry.fill(0.),
            7|8=>for i in 0..128 {
                let angle=i as f32*if s.p.mode==7 {std::f32::consts::TAU}else{std::f32::consts::PI}/128.;
                s.a[i]=if s.p.mode==7 {Vec3::new(angle.sin(),0.,angle.cos())}else{Vec3::new((angle*30.).sin()*angle.sin(),angle.cos(),(angle*30.).cos()*angle.sin())};
                s.stars[i].active=true;
            },
            13=>for i in 0..128 {s.walk(crt);s.a[i]=s.direction;},
            3..=6|9..=12|14..=20|24..=28=>{},
            _=>unreachable!(),
        }
        Ok(s)
    }
    pub(super) fn configure(&mut self,c:EffectConfig)->Result<()> {
        if let Some(d)=c.duration {ensure!(d.is_finite(),"invalid Stars duration");self.p.duration=d;}
        if c.start_color.is_some()||c.stop_color.is_some() {self.template.words.resize(self.template.words.len().max(32),0);}
        for (color,indices,k) in [(c.start_color,[19,20,21,18],0),(c.stop_color,[23,24,25,22],1)] {
            if let Some(color)=color {
                ensure!(color.iter().all(|v|v.is_finite()),"invalid Stars color");
                for (i,v) in indices.into_iter().zip(color) {self.template.words[i]=v.to_bits();}
                self.p.colors[k]=[color[3],color[0],color[1],color[2]];
            }
        }
        Ok(())
    }
    pub(super) fn update_source(&mut self,source:Mat4)->Result<()> {self.source=sprites::connector(&self.template,source)?;Ok(())}
    pub(super) fn update_anchors(&mut self,source:Mat4,target:Mat4)->Result<()> {self.target=target;self.update_source(source)}
    pub(super) fn required_attractors(&self)->&[i32] {if matches!(self.p.mode,21|22) {&BONES}else{&[]}}
    pub(super) fn set_attractor(&mut self,id:i32,position:Vec3) {for (slot,bone) in self.chain.iter_mut().zip(BONES) {if id==bone {*slot=Some(mirror(position));}}}
    pub(super) fn needs_source_geometry(&self)->bool {self.p.mode==15}
    pub(super) fn set_source_root(&mut self,root:Mat4) {self.body_root=Some(root);}
    pub(super) fn set_ground_height(&mut self,height:f32) {self.ground=height.is_finite().then_some(height);}
    pub(super) fn set_source_geometry(&mut self,vertices:&[Vertex],_indices:&[u32],crt:&mut CrtRand)->Result<()> {
        if self.p.mode!=15 {return Ok(());}
        ensure!(!vertices.is_empty(),"Stars body callback requires posed vertices");
        // GC100f6eb6: callback visits every max(total/31,1)th vertex,
        // with a fresh rand()%30 starting offset on each native pose callback.
        let stride=(vertices.len()/31).max(1);
        let start=crt.rand() as usize%30;
        for i in (start..vertices.len()).step_by(stride) {
            let slot=i/stride;
            if slot<30 {self.sampled[slot]=mirror(Vec3::from_array(vertices[i].pos));self.normals[slot]=mirror(Vec3::from_array(vertices[i].normal));}
        }
        self.body_ready=true;
        Ok(())
    }
    pub(super) fn terminate_gracefully(&mut self) {self.stop=true;}
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {
        vec![(Some(self.p.material as usize),(0..128u32).flat_map(|i|[i*4,i*4+1,i*4+2,i*4+1,i*4+2,i*4+3]).collect(),512)]
    }
    pub(super) fn blends(&self)->Vec<Blend> {vec![if self.p.mode==25 {Blend::AlphaBlend}else{Blend::Additive}]}
    #[allow(clippy::approx_constant)]
    fn walk(&mut self,r:&mut CrtRand) {
        self.axis=(self.axis+sprites::sphere_point(r)*0.15).normalize();
        // GC10051b95 wraps before taking the half-angle. GC1007c09c has
        // reversed Hamilton cross terms; do not substitute a glam transform.
        let angle=self.p.radius;
        let half=if angle>=0. && (angle as f64)<std::f64::consts::TAU {angle*0.5}else{
            let turns=(angle as f64/6.2831854820251465) as f32;
            ((turns as f64-turns.floor() as f64)*3.1415927410125732) as f32
        };
        let v=self.axis*half.sin();let q=[v.x,v.y,v.z,half.cos()];
        let mul=|a:[f32;4],b:[f32;4]| [
            a[3]*b[0]+b[1]*a[2]-b[2]*a[1]+a[0]*b[3],
            a[3]*b[1]-b[0]*a[2]+b[2]*a[0]+a[1]*b[3],
            a[1]*b[0]-a[0]*b[1]+b[2]*a[3]+b[3]*a[2],
            a[3]*b[3]-(a[1]*b[1]+b[0]*a[0]+b[2]*a[2]),
        ];
        let inv=1./(q[0]*q[0]+q[1]*q[1]+q[2]*q[2]+q[3]*q[3]);
        let d=self.direction;
        let out=mul(mul(q,[d.x,d.y,d.z,0.]),[-q[0]*inv,-q[1]*inv,-q[2]*inv,q[3]*inv]);
        self.direction=Vec3::new(out[0],out[1],out[2]).normalize();
    }
    fn color(&self,u:f32)->[f32;4] {authored_color(&self.template,u)}
    fn origin(&self)->Vec3 {mirror(self.source.w_axis.truncate())}
    fn hit(&self)->(Vec3,Vec3) {(self.origin(),mirror(self.target.w_axis.truncate()))}
    fn curve(&self,r:f32)->Vec3 {
        // GC101051e7 / 101050d0: cubic control points from actual locators.
        let (start,end)=self.hit();
        let mut d=end-start;
        let length=d.length();
        if length==0. {return start;}
        let mut up=Vec3::Y;
        if d.x==0. && d.z==0. {d*=0.3333;up=Vec3::X;}
        let v=d/length;
        let cross=v.cross(up);
        let n=cross.normalize();
        let forward=-mirror(self.source.z_axis.truncate());
        let k=if n.dot(forward)>0. {-0.4}else{0.4};
        let c1=start+forward*((length as f64*0.33000001311302185) as f32);
        let c2=(c1+end)*0.5+n*((forward.dot(v)-1.)*length*k);
        let q=1.-r;let q2=q*q;let r2=r*r;
        ((start*(q2*q)+c1*((r*3.)*q2))+c2*((r2*3.)*q))+end*(r2*r)
    }
    fn project(&self,j:usize,p:Vec3)->Option<Vec3> {
        let start=self.chain[j*2]?;
        let d=self.chain[j*2+1]?-start;
        let n=d.length_squared();
        if n==0. {return Some(start);}
        Some(start+d*((p-start).dot(d)/n).clamp(0.,1.))
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn vertices(&mut self,time:f32,_camera:Vec3,right:Vec3,up:Vec3,_gc:&mut R250,_ds:&mut R250,crt:&mut CrtRand)->Result<Option<Vec<Vec<Vertex>>>> {
        if !self.alive {return Ok(None);}
        ensure!(time.is_finite()&&time>=0.,"invalid Stars time");
        if self.p.mode==15&&(!self.body_ready||self.body_root.is_none()) {
            if self.stop {self.alive=false;return Ok(None);}
            return Ok(Some(vec![Vec::new()]));
        }
        if self.p.duration>=0. && time>=self.p.duration && !self.stop {
            if matches!(self.p.mode,1|3..=6|9|11|12|14|15|21..=25) {self.stop=true;}else {self.alive=false;return Ok(None);}
        }
        self.ticks=self.ticks.wrapping_add(1);
        self.process(time,crt)?;
        if !self.alive {return Ok(None);}
        let (_,cols,rows,_,_)=materials::MATERIALS[self.p.material as usize];
        let empty=Vertex {pos:[0.;3],normal:[0.,0.,1.],uv:[0.;2],color:[0.;4]};
        let mut vertices=Vec::with_capacity(512);
        for star in &self.stars {
            if !star.active {vertices.extend([empty;4]);continue;}
            let center=mirror(star.position);
            let x=right*(star.width*0.5);let y=up*(star.height*0.5);
            let u=(star.frame%cols as i32) as f32/cols as f32;
            let v=(star.frame/cols as i32) as f32/rows as f32;
            quad(&mut vertices,[center-x-y,center+x-y,center-x+y,center+x+y],
                [[u,v+1./rows as f32],[u+1./cols as f32,v+1./rows as f32],[u,v],[u+1./cols as f32,v]],render_color(star.color));
        }
        Ok(Some(vec![vertices]))
    }
    fn process(&mut self,t:f32,r:&mut CrtRand)->Result<()> {
        let m=self.p.mode;let o=self.origin();let (start,end)=self.hit();
        let s=self.p.size;let radius=self.p.radius;let n=self.p.count;let base=self.p.shape[0];let q=t/self.p.duration;
        let life=match m {10|12=>s,1|21=>1.2,_=>self.p.lifetime_ms as f32/1000.};
        if m==25 {ensure!(self.ground.is_some(),"Stars mode25 requires native ground height");}
        if matches!(m,21|22) {ensure!(self.chain.iter().all(Option::is_some),"Stars connector chain is unavailable");}
        if m==13 {for _ in 0..7 {self.walk(r);self.head=(self.head+1)%128;self.a[self.head]=self.direction;}}
        let mut budget=match m {1|3..=6|9|11|12|21=>2u32,10=>5,15|18|26|28=>10,16|17|19|20|27=>15,14|22|25=>n,_=>0};
        let mut occupied=0u32;
        for i in 0..128 {
            if matches!(m,9|25)&&i%2!=0 {continue;}
            let delay=if m==6 {(i&7) as f32/9.5}else{0.};
            if matches!(m,0|2|7|8|13|24) {
                let mut frame=self.stars[i].frame;
                let (position,width,color,active)=match m {
                    0=>{frame=((t*10.) as i32).min(15);ensure!(frame>=0,"negative Stars frame");(o+self.a[i]*(2.*(1.6-t).sqrt()),(1.5-t)*0.25*20./FRAMES[frame as usize],self.stars[i].color,true)},
                    2=>{
                        frame=(((1.-q)*15.) as i32).clamp(1,15);let width=s*(1.-q)*20./EXPLOSION[frame as usize];
                        const RGB:[[f32;3];8]=[[1.,0.7,0.7],[1.,1.,0.],[1.,0.,1.],[0.,1.,1.],[0.,0.,1.],[0.,1.,0.],[1.,0.,0.],[0.7,1.,0.7]];
                        let mut color=self.color(q);for (j,v) in color[..3].iter_mut().enumerate() {*v=RGB[i&7][j]+(self.p.colors[1][j+1]-RGB[i&7][j])*q;}frame-=(i&1) as i32;
                        (o+self.a[i]*(q.sqrt()*0.7+0.6),width,color,true)
                    },
                    7|8=>{let x=n as f32*t/self.p.duration;let phase=x-x.floor();let root=phase.sqrt();(o+self.a[i]*(root*0.01*self.p.lifetime_ms as f32),s*root+radius,self.color(phase),true)},
                    13=>{let phase=((self.head+128-i)%128) as f32/128.;(o+self.a[i]*(s*phase),base,self.color(phase),hump(q)>phase)},
                    24=>(if i==0 {start}else{self.stars[i-1].position+(end-start)/127.},s,self.color(q),true),
                    _=>unreachable!(),
                };
                self.stars[i]=Star{position,width,height:width,color,frame,active};continue;
            }
            if self.expiry[i]+delay<=t {
                self.stars[i].active=false;
                if self.stop || (m==12&&i%4!=0) {continue;}
                if m==23 {if occupied>=(q*n as f32) as u32 {continue;}}else if budget==0 {continue;}
                let mut expiration=t+life;
                match m {
                    1|21=>{self.b[i]=Vec3::ZERO;self.c[i]=o+sprites::sphere_point(r);self.a[i]=Vec3::splat(2.);},
                    3=>{let mut v=inside(r,true);v.x*=radius;v.z*=radius;self.c[i]=o+v;self.b[i]=Vec3::new(v.z,0.,-v.x);},
                    4=>{self.c[i]=o;self.b[i]=(inside(r,true)+Vec3::Y*3.)*radius;},
                    5=>{self.c[i]=o-Vec3::Y*1.2;self.b[i]=mirror(circle(r))*radius;},
                    6|11=>{let v=mirror(circle(r))*radius;self.c[i]=o+v-Vec3::Y;self.b[i]=Vec3::new(v.z,0.5,-v.x);},
                    9=>self.c[i]=o+inside(r,false)*radius,
                    10=>self.c[i]=o+sprites::sphere_point(r)*radius,
                    12=>{let v=sprites::sphere_point(r);for j in 0..4 {self.c[i+j]=v;self.expiry[i+j]=t+life-(j as f32*0.1)*life;}expiration=self.expiry[i];},
                    14=>{self.phase+=radius;self.c[i]=Vec3::new(self.phase.sin(),0.,self.phase.cos());},
                    15=>{let k=self.sample_index;let root=self.body_root.expect("native body root");let rotation=root.to_scale_rotation_translation().1;self.c[i]=mirror(root.transform_point3(mirror(self.sampled[k])));self.b[i]=mirror(rotation*mirror(self.normals[k]*radius));self.sample_index=(k+1)%30;},
                    16=>{let point=inside(r,true);let u=r.rand() as f32/32768.;self.c[i]=start*(1.-u)+end*u+point*radius;},
                    17|27=>{
                        let mut d=end-start;if m==17&&d.length_squared()<1e-5 {d.x+=0.001;}
                        let p=perpendicular(d);let normal=p.cross(d).normalize();
                        let u=r.rand() as f32/32768.;let z=1.-2.*u;let theta=(d.length() as f64*(u as f64+u as f64)+4.*q as f64+radius as f64) as f32;
                        self.c[i]=start+d*u+(normal*theta.cos()+p*theta.sin())*((0.5*(1.-z as f64*z as f64)) as f32);
                    },
                    18..=20=>{self.c[i]=start*(1.-q)+end*q+inside(r,true)*radius;if m==19 {expiration=t+life*(0.9+(r.rand()&2047) as f32*0.0001);}},
                    22=>{let j=r.rand() as usize%6;let u=r.rand() as f32/32768.;let c=mirror(circle(r));let a=self.chain[j*2].expect("checked chain");let d=self.chain[j*2+1].expect("checked chain")-a;let c=if d.length_squared()!=0. {let p=perpendicular(d);c.x*p+c.z*p.cross(d).normalize()}else{c};self.c[i]=a+d*u+c*s;self.a[i]=Vec3::splat(j as f32);},
                    23=>{let mut v=inside(r,false);v.y=s;self.c[i]=o+v;},
                    25=>{self.b[i]=inside(r,true)*radius+Vec3::Y*1.1;self.c[i]=o;},
                    26=>{let u=r.rand() as f32/32768.;self.c[i]=self.curve(u);},
                    28=>self.c[i]=start*(1.-q)+end*q,
                    _=>unreachable!(),
                }
                if matches!(m,16..=20|22|26..=28) {self.b[i]=Vec3::ZERO;}
                self.expiry[i]=expiration;budget=budget.saturating_sub(1);occupied+=1;continue;
            }
            let u=(t+life-self.expiry[i])/life;let mut width=s;let mut height=s;let mut frame=0;let mut color=self.color(u);let mut position=self.c[i];
            match m {
                1|21=>{
                    let target=if m==21 {let j=self.a[i].x as usize;let k=(self.ticks as usize%5+j)%6;let a=self.project(j,self.c[i]).expect("checked chain");let b=self.project(k,self.c[i]).expect("checked chain");if (a-self.c[i]).length_squared()>(b-self.c[i]).length_squared() {self.a[i]=Vec3::splat(k as f32);b}else{a}}else{o};
                    let force=target-self.c[i]-self.b[i]*2.;self.b[i]+=force*0.1;self.c[i]+=self.b[i]*0.1;frame=15-(u*15.99) as i32;ensure!((0..16).contains(&frame),"native Stars frame outside size table");width=u*0.01*FRAMES[frame as usize];height=width;position=self.c[i];
                },
                3|5|6|11=>{
                    let mut force=o-self.c[i];if m==6 {force.y=0.;}self.b[i]+=force*if m==3 {0.1}else{n as f32/100.};self.c[i]+=self.b[i]*match m {5=>0.2,6=>0.15,_=>0.1};
                    let phase=if m==6 {(life+t-self.expiry[i])/(life+delay)}else{u};frame=if m==6 {15-(phase*16.) as i32}else{15-((self.expiry[i]-t)*16./life) as i32};frame=frame.clamp(0,15);width=(phase+0.2)*s*(1.-phase*phase);height=width;color=self.color(phase);position=self.c[i];
                },
                4=>{self.b[i]=self.b[i]*0.95-Vec3::Y*0.1;self.c[i]+=self.b[i]*0.1;frame=(((self.expiry[i]-t)*64./life) as i32).clamp(0,63);width=(2.-u)*s;height=width;position=self.c[i];},
                9=>{let h=0.5-(t+life-self.expiry[i])*s;width=base+if h>0. {h*n as f32*0.01}else{0.};height=width;position=self.c[i]+Vec3::Y*(h+1.5);},
                10=>{let h=2.*((self.expiry[i]-t)/life-0.5);width=(1.-h*h)*(n as f32*0.01)+base;height=width;let mask=((hump(q)*255.) as u32)<<24|0xffffff;color=unpack(if self.p.lifetime_ms!=0 {PALETTE[i&7]&mask}else{(packed(self.color(q))|0xff000000)&mask});},
                12=>{width=(1.-u)*(n as f32*0.01)+base;height=width;color=unpack(PALETTE[(i>>2)&7]);position=o+(self.c[i]*radius)*u;},
                14=>{let phase=if self.stop {1.}else{q};position=o+Vec3::new(self.c[i].x*s*u,self.c[i].y+2.*phase-1.,self.c[i].z*s*u);width=base;height=base;},
                15=>{self.b[i]*=0.96;self.c[i]+=self.b[i]*0.1;width=(1.1-u)*s;height=width;position=self.c[i];},
                16..=19|26..=28=>{
                    width=s*hump(u);height=width;frame=15-(u*15.99) as i32;
                    if matches!(m,17|27) {color=self.color(q);}
                    if m==28 {position=start*(1.-q)+end*q;}
                },
                20=>frame=(u*63.99) as i32,
                22=>{let force=self.project(self.a[i].x as usize,self.c[i]).expect("checked chain")-self.c[i];self.b[i]+=force*0.1;self.c[i]+=self.b[i]*0.1;width=(0.7*u+0.3)*radius;height=width;frame=(u*15.99) as i32;if force.length_squared()<0.04 {self.expiry[i]=0.;}position=self.c[i];},
                23=>{width=(0.9*u+0.1)*radius;height=width;position=o*(u*u)+self.c[i]*(1.-u*u);},
                25=>{self.b[i]*=0.99;self.b[i].y-=0.025;self.c[i]+=self.b[i]*0.1;height=(2.5*hump(u)+1.)*s;frame=(u*63.99) as i32;if self.c[i].y<self.ground.expect("checked height")-0.5 {self.expiry[i]=0.;}position=self.c[i];},
                _=>unreachable!(),
            }
            self.stars[i]=Star{position,width,height,color,frame,active:true};occupied+=1;
        }
        if self.stop&&(matches!(m,0|2|7|8|13|24)||occupied==0) {self.alive=false;}
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authored_17100_parameters_and_all_native_modes() -> Result<()> {
        let mut t = Template { kind: 2004, words: vec![5,0,0,0,0,0,0,1000,3212836864,33,16,1053609165,1041865114,1082130432,1077936128,1065353216,1077936128,1077936128,1065353216,1065353216,1050253722,1050253722,1065353216,1065353216,1050253722,1050253722,1065353216,0,1045220557,1053609165,300,6] };
        let p = StarsParameters::decode(&t)?;
        assert_eq!((t.words[0],p.material,p.mode,p.lifetime_ms,p.count),(5,33,16,300,6));
        assert_eq!((p.duration,p.offset,p.size,p.radius),(1.0,0.0,0.2,0.4));
        assert_eq!(p.shape,[0.4,0.15,4.0,3.0,1.0,3.0,3.0]);
        assert_eq!(p.colors,[[1.0,1.0,0.3,0.3],[1.0,1.0,0.3,0.3]]);
        for mode in 0..=28 {t.words[10]=mode;assert_eq!(StarsParameters::decode(&t)?.mode,mode);}
        t.words[10]=29;
        assert!(StarsParameters::decode(&t).is_err());
        Ok(())
    }

    #[test]
    fn native_mode16_stops_emitting_and_drains_existing_slots()->Result<()> {
        let mut words=vec![0;32];
        words[0]=5;words[9]=33;words[10]=16;words[26]=1f32.to_bits();
        words[28]=0.2f32.to_bits();words[29]=0.4f32.to_bits();words[30]=300;words[31]=6;
        let t=Template{kind:2004,words};
        let mut gc=R250::new(1);let mut ds=R250::new(2);let mut crt=CrtRand::new(3);
        let mut fx=StarsEffect::new(&t,Mat4::IDENTITY,Mat4::from_translation(Vec3::Y),0,&mut gc,&mut ds,&mut crt)?;
        fx.process(0.,&mut crt)?;
        assert_eq!(fx.expiry.iter().filter(|v|**v>0.).count(),15);
        fx.process(0.1,&mut crt)?;
        assert!(fx.stars.iter().any(|p|p.active));
        let expiry=fx.expiry;
        fx.terminate_gracefully();
        fx.process(0.2,&mut crt)?;
        assert!(fx.alive);
        assert_eq!(fx.expiry,expiry);
        fx.process(0.5,&mut crt)?;
        assert!(!fx.alive);
        assert!(fx.stars.iter().all(|p|!p.active));
        Ok(())
    }

    #[test]
    fn authored_12600_mode28_follows_current_hit_center()->Result<()> {
        let t=Template{kind:2004,words:vec![5,0,0,0,0,0,0,1000,3212836864,33,28,1045220557,1045220557,1082130432,1053609165,1053609165,1053609165,1053609165,1065353216,1063675494,1063675494,1065353216,1065353216,1063675494,1063675494,1065353216,1065353216,0,1045220557,0,300,6]};
        let mut gc=R250::new(1);let mut ds=R250::new(2);let mut crt=CrtRand::new(3);
        let mut fx=StarsEffect::new(&t,Mat4::IDENTITY,Mat4::from_translation(Vec3::Y),0,&mut gc,&mut ds,&mut crt)?;
        fx.process(0.,&mut crt)?;
        fx.process(0.1,&mut crt)?;
        assert_eq!(fx.stars[0].position,Vec3::Y*0.1);
        fx.update_anchors(Mat4::IDENTITY,Mat4::from_translation(Vec3::Y*2.))?;
        fx.process(0.2,&mut crt)?;
        assert_eq!(fx.stars[0].position,Vec3::Y*0.4);
        assert_eq!(packed([0.3;4]),0x4c4c4c4c);
        Ok(())
    }
}
