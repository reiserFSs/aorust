//! Gamecode Species_c 1011d716/1011d88f and Vehicle.dll 1000e3d3.
use ao_formats::wildlife::LifeKind;
use ao_formats::playfield::collision::{Aligned, Body, SurfaceState};
use ao_scene::CrtRand;
use glam::{Quat, Vec3};
use std::sync::Arc;
#[path="native_life_air.rs"] mod air;
#[path="native_life_animals.rs"] mod animals;

pub struct CharacterSensor { pub pos: Vec3, pub radius: f32 }
pub trait Environment {
    fn align(&self, old: Vec3, new: Vec3, body: &Body, state: &mut SurfaceState) -> Aligned;
    fn ground(&self, pos: Vec3) -> Option<(Vec3, Vec3)>;
    fn ground_ray(&self, origin: Vec3) -> Option<(Vec3, Vec3)>;
    fn alignment(&self, pos: Vec3) -> Vec3;
    fn liquid(&self, pos: Vec3) -> Option<f32>;
    fn district(&self, pos: Vec3) -> Option<usize>;
    fn obstacle(&self, pos: Vec3, ahead: Vec3) -> Option<(Vec3, Vec3)>;
    fn characters(&self) -> &[CharacterSensor];
    fn dangers(&self) -> &[CharacterSensor];
    fn corpses(&self) -> &[Vec3];
    fn wind(&self) -> Vec3;
    fn landing(&self, pos: Vec3, radius: f32) -> Option<Vec3>;
}
#[derive(Clone, Copy)]
pub struct Neighbor { pub id: u32, pub pos: Vec3, pub vel: Vec3, pub state: i32, pub group: Option<u32> }
pub struct Context<'a> { pub dt: f32, pub environment: &'a dyn Environment, pub neighbors: &'a [Neighbor] }
#[derive(Clone, Copy, Debug)]
pub enum Steering { None, Halt, Force(Vec3) }
pub struct Species {
    pub id: u32, pub kind: LifeKind, pub catalog_index: usize, pub district: usize,
    pub pos: Vec3, pub vel: Vec3, pub rotation: Quat, pub wander: Vec3,
    pub last_in_district: Vec3, pub target: Vec3, pub state: i32, pub timer: f32, pub counter: u32,
    pub max_speed: f32, pub max_force: f32, pub mass: f32, pub brake_distance: f32,
    pub body_radius: f32, pub group: Option<u32>, pub animation: usize,
    pub pending_animation: Option<usize>, pub animation_time: f32, pub animation_serial: u64,
    pub liquid_escape: bool, pub surface_hug: bool, pub falling: bool,
    pub descending_animation: bool, pub lifespan: f32, pub elapsed: f32, pub alive: bool, pub visible: bool,
    gravity_velocity: f32, airborne: bool, surface: SurfaceState, surface_up: Vec3, rng: CrtRand,
}
impl Species {
    pub fn new(kind: LifeKind, catalog_index: usize, pos: Vec3, district: usize, rng: CrtRand) -> Self {
        let mut s=Self {id:0,kind,catalog_index,district,pos,vel:Vec3::ZERO,rotation:Quat::IDENTITY,wander:Vec3::ZERO,last_in_district:Vec3::ZERO,target:Vec3::ZERO,state:0,timer:0.,counter:0,max_speed:0.,max_force:0.,mass:1.,brake_distance:1.,body_radius:0.,group:None,animation:0,pending_animation:None,animation_time:0.,animation_serial:0,liquid_escape:false,surface_hug:false,falling:false,descending_animation:false,lifespan:-1.,elapsed:0.,alive:true,visible:false,gravity_velocity:0.,airborne:false,surface:SurfaceState::default(),surface_up:Vec3::Y,rng};
        // Species_c 101173ba sets liquid medium 1; Fish 10118edb overrides it to 3.
        s.surface.medium = if kind == LifeKind::Fish { 3 } else { 1 };
        match kind {
            LifeKind::Bird|LifeKind::Rabbit|LifeKind::Fish=>animals::configure(&mut s),
            LifeKind::Butterfly|LifeKind::Fly|LifeKind::Hoverbot|LifeKind::Pollen=>air::configure(&mut s),
            LifeKind::Beetle=>{s.max_speed=0.8;s.max_force=10.;s.mass=0.1;s.brake_distance=0.1;s.falling=true;s.surface_hug=true;s.state=1;s.animation=1;},
            LifeKind::Cleanerbot=>{s.max_speed=0.6;s.max_force=8.;s.mass=3.;s.brake_distance=0.1;s.falling=true;s.surface_hug=true;s.counter=(s.random()*32767.) as u32%5;s.animation=1;},
            LifeKind::Trash=>{s.max_speed=3.;s.max_force=14.;s.mass=0.2;s.brake_distance=0.1;},
            LifeKind::TumbleWeed=>{s.max_speed=5.;s.max_force=1.;s.mass=0.5;s.brake_distance=0.1;s.falling=true;},
        }
        s
    }
    /// SetRelPos placement resolves support before the first Vehicle substep.
    pub fn place(&mut self, environment: &dyn Environment) { self.resolve(self.pos, self.pos, true, environment); self.orient(environment); }
    fn resolve(&mut self, old: Vec3, next: Vec3, teleport: bool, environment: &dyn Environment) {
        self.surface.radius = self.body_radius;
        let forward = self.rotation * Vec3::Z;
        self.surface.heading = forward.x.atan2(forward.z);
        let body = Body { falling_enabled: self.falling, airborne: self.airborne, vy: self.gravity_velocity, teleport };
        let aligned = environment.align(old, next, &body, &mut self.surface);
        self.pos = Vec3::from_array(aligned.pos);
        if self.falling {
            self.airborne = aligned.airborne;
            if !self.airborne { self.gravity_velocity = 0.; }
        }
    }
    pub fn random(&mut self)->f32 { self.rng.rand() as f32/32767. }
    pub fn animate(&mut self,index:usize,queued:bool) { if queued {self.pending_animation=Some(index)} else {self.animation=index;self.pending_animation=None;self.animation_time=0.;self.animation_serial=self.animation_serial.wrapping_add(1);} }
    pub fn animation_finished(&self)->bool { self.animation==0 }
    pub fn halt(&mut self) {self.vel=Vec3::ZERO;}
    pub fn arrive(&self,target:Vec3,radius:f32)->Steering {
        let d=target-self.pos;let d2=d.length_squared();
        if d2<radius*radius||d2<0.01 {Steering::Halt} else {let l=d2.sqrt();Steering::Force((d/l*self.max_speed.min(l/self.brake_distance*self.max_speed)-self.vel)*self.mass*4.)}
    }
    pub fn wander3(&mut self,radius:f32,jitter:f32,forward:f32)->Vec3 {self.wander_step(radius,jitter,forward,false)}
    pub fn wander2(&mut self,radius:f32,jitter:f32,forward:f32)->Vec3 {self.wander_step(radius,jitter,forward,true)}
    fn wander_step(&mut self,radius:f32,jitter:f32,forward:f32,flat:bool)->Vec3 {
        let x=self.random()-0.5;let y=if flat {0.} else {self.random()-0.5};let z=self.random()-0.5;
        self.wander+=Vec3::new(x,y,z).normalize_or_zero()*jitter;
        let centre=self.vel.normalize_or_zero()*forward;
        self.wander=centre+(self.wander-centre).normalize_or_zero()*(radius-jitter);
        self.wander
    }
    pub fn constrain_district(&mut self,c:&Context)->Option<Vec3> {
        if c.environment.district(self.pos)==Some(self.district) {self.last_in_district=self.pos;None}
        else {Some((self.last_in_district-self.pos).normalize_or_zero()*self.max_force)}
    }
    pub fn obstacle(&self,c:&Context)->Option<Vec3> {let ahead=self.vel+self.rotation*Vec3::Z;c.environment.obstacle(self.pos,ahead).map(|(hit,normal)|normal*(self.max_force*hit.distance(self.pos)/ahead.length()))}
    pub fn flee(&self,c:&Context,radius:f32)->Option<Vec3> {
        c.environment.characters().iter().filter(|p|p.pos.distance_squared(self.pos)<radius*radius).min_by(|a,b|a.pos.distance_squared(self.pos).total_cmp(&b.pos.distance_squared(self.pos))).map(|p|((self.pos-p.pos).normalize_or_zero()*self.max_speed-self.vel)*self.mass*4.)
    }
    pub fn accumulate(&self,force:&mut Vec3,candidate:Vec3)->f32 {
        let magnitude=force.length();let remaining=1.-magnitude;let requested=candidate.length()/self.max_force;let contribution=requested.min(remaining);
        if requested>=0.001||remaining<requested { *force+=candidate.normalize_or_zero()*contribution; return magnitude+contribution; }
        magnitude
    }
    fn neighbors<'a>(&'a self,c:&'a Context,radius:f32,angle:f32)->impl Iterator<Item=&'a Neighbor> {
        c.neighbors.iter().filter(move |n|self.group.is_some()&&n.group==self.group&&n.id!=self.id&&n.pos.distance_squared(self.pos)<=radius*radius&&(angle.is_infinite()||(self.vel.length_squared()>0.&&(n.pos-self.pos).normalize_or_zero().dot(self.vel.normalize_or_zero()).acos()<=angle)))
    }
    pub fn separation(&self,c:&Context,radius:f32,angle:f32)->Option<Vec3> {
        let mut force=Vec3::ZERO;let mut found=false;for n in self.neighbors(c,radius,angle) {found=true;let d=self.pos-n.pos;if angle.is_infinite()||n.state!=2 {force+=d.normalize_or_zero()*(radius*radius/(2.*d.length_squared()));}}found.then_some(force)
    }
    pub fn cohesion(&self,c:&Context,radius:f32,angle:f32)->Option<Vec3> {let mut sum=Vec3::ZERO;let mut count=0;for n in self.neighbors(c,radius,angle){sum+=n.pos;count+=1;} (count>0).then(||sum/count as f32-self.pos)}
    pub fn alignment(&self,c:&Context,radius:f32,angle:f32)->Option<Vec3> {let mut sum=Vec3::ZERO;let mut count=0;for n in self.neighbors(c,radius,angle){sum+=n.vel;count+=1;} (count>0).then(||sum/count as f32-self.vel)}
    pub fn tick(&mut self,dt:f32,environment:&dyn Environment,neighbors:&[Neighbor],zone_visible:bool,distance_visible:bool) {
        self.visible=zone_visible&&distance_visible;
        if !zone_visible {self.lifespan=0.;self.elapsed=0.;}
        if !self.alive||!self.visible||dt<=0.||dt>4. {self.age(dt);return;}
        let mut left=dt;
        while left>0. {let h=left.min(0.4);let c=Context {dt:h,environment,neighbors};
            let steering=match self.kind {LifeKind::Bird|LifeKind::Rabbit|LifeKind::Fish=>animals::steer(self,&c),LifeKind::Butterfly|LifeKind::Fly|LifeKind::Hoverbot|LifeKind::Pollen=>air::steer(self,&c),_=>self.steer_other(&c)};
            let force=match steering {Steering::Force(v)=>v,Steering::Halt=>{self.halt();Vec3::ZERO},Steering::None=>Vec3::ZERO};
            // Vehicle+0x52 is airborne, independently of EnableFalling (+0x50).
            if self.airborne {self.gravity_velocity=(self.gravity_velocity-20.*h).clamp(-50.,50.);} else if self.surface_hug {self.gravity_velocity=0.;self.vel.y=0.;}
            self.vel+=(force.clamp_length_max(self.max_force)/self.mass)*h;
            self.vel=self.vel.clamp_length_max(self.max_speed);
            let old = self.pos;
            let next = old + (self.vel + Vec3::Y * self.gravity_velocity) * h;
            self.resolve(old, next, false, environment);
            self.orient(environment);
            left-=h;
        }
        self.age(dt);
    }
    fn orient(&mut self, environment: &dyn Environment) {
        let mut forward = if self.vel.length_squared() > 0. { self.vel } else { self.rotation * Vec3::Z };
        let mut up = Vec3::Y;
        if matches!(self.kind, LifeKind::Beetle | LifeKind::Cleanerbot | LifeKind::TumbleWeed) {
            self.surface_up = (self.surface_up * 0.9 + environment.alignment(self.pos) * 0.1).normalize_or_zero();
            up = self.surface_up;
            if up.y <= 0. { up = Vec3::Y; }
            forward.y -= forward.dot(up) / up.y;
        } else {
            forward.y = 0.;
        }
        if forward.length_squared() == 0. { forward = Vec3::Z - up * Vec3::Z.dot(up); }
        forward = forward.normalize_or_zero();
        let right = up.cross(forward).normalize_or_zero();
        self.rotation = Quat::from_mat3(&glam::Mat3::from_cols(right, forward.cross(right), forward));
    }
    fn age(&mut self,dt:f32) {if self.lifespan>=0. {self.elapsed+=dt;if self.elapsed>self.lifespan {self.alive=false;self.visible=false;}}}
    fn steer_other(&mut self,c:&Context)->Steering {
        match self.kind {
            LifeKind::TumbleWeed=>{let mut force=Vec3::ZERO;self.accumulate(&mut force,c.environment.wind());Steering::Force(force*self.max_force)},
            LifeKind::Trash=>{
                if self.random()<0.005 {self.state=1-self.state;self.animate(self.state as usize,true);}
                let x=self.random();let y=self.random();let z=self.random();
                let mut v=c.environment.wind()*0.1+Vec3::new((x-0.5)*4.,(y-0.5)*4.,(z-0.5)*4.);
                if v.x>2. {v.x=1.7;} else if v.x< -1.7 {v.x = -1.7;}
                if v.z>2. {v.z=1.7;} else if v.z< -1.7 {v.z = -1.7;}
                v.y-=0.5;
                let mut force=Vec3::ZERO;self.accumulate(&mut force,v);Steering::Force(force*self.max_force)
            },
            LifeKind::Beetle|LifeKind::Cleanerbot=>{
                let resting_before=if self.kind==LifeKind::Beetle {self.state==0} else {self.state==1};
                if self.kind==LifeKind::Cleanerbot {self.counter+=1;}
                self.timer+=c.dt;
                if self.timer>0.5 {self.timer=0.;let probability=if self.kind==LifeKind::Beetle {if self.state==0 {0.08} else {0.06}} else {if self.state==0 {0.08} else {0.14}};
                    let water=c.environment.liquid(self.pos).is_some_and(|h|h>0.);
                    if !(self.kind==LifeKind::Beetle&&self.state==1&&water)&&self.random()<probability {self.state=1-self.state;self.animate(if self.kind==LifeKind::Beetle {self.state as usize} else {(1-self.state) as usize},false);if (self.kind==LifeKind::Beetle&&self.state==0)||(self.kind==LifeKind::Cleanerbot&&self.state==1) {self.halt();}}
                    if self.kind==LifeKind::Beetle&&self.state==1 {if water {if !self.liquid_escape {self.liquid_escape=true;self.lifespan=10.;self.elapsed=0.;}self.wander=(self.target-self.pos).normalize_or_zero()*self.max_force;} else {self.liquid_escape=false;self.lifespan = -1.;self.elapsed=0.;self.target=self.pos;}}
                }
                let resting=if self.kind==LifeKind::Beetle {self.state==0} else {self.state==1};if resting||resting_before {return Steering::None;}
                let mut force=Vec3::ZERO;
                if self.kind==LifeKind::Cleanerbot&&self.counter>=5 {self.counter=0;if let Some(v)=self.obstacle(c) {self.wander=v; if self.accumulate(&mut force,v)>0.99 {return Steering::Force(force*self.max_force);}}}
                if self.kind==LifeKind::Beetle&&self.liquid_escape&&self.accumulate(&mut force,self.wander)>0.99 {return Steering::Force(force*self.max_force);}
                if let Some(v)=self.constrain_district(c) {self.wander=v*0.5;if self.accumulate(&mut force,v)>0.99 {return Steering::Force(force*self.max_force);}}
                let forward=if self.kind==LifeKind::Beetle {1.2} else {1.};let v=self.wander2(5.,1.4,forward);self.accumulate(&mut force,v);Steering::Force(force*self.max_force)
            },
            _=>unreachable!(),
        }
    }
}
/// SpeciesBody_c 1011815f/1011821c, event times already converted from milliseconds.
pub struct Animation { pub boundaries: Arc<[f32]>, pub total: f32, pub index: usize, pub time: f32, pub pending: Option<usize> }
impl Animation {
    pub fn new(boundaries:Arc<[f32]>,total:f32)->Self {Self {boundaries,total,index:0,time:0.,pending:None}}
    pub fn select(&mut self,index:usize,queued:bool) {if queued {self.pending=Some(index)} else {self.index=index.min(self.boundaries.len().saturating_sub(1));self.time=self.boundaries.get(self.index).copied().unwrap_or(0.);self.pending=None;}}
    pub fn tick(&mut self,dt:f32) {self.time+=dt;let end=self.boundaries.get(self.index+1).copied().unwrap_or(self.total);if self.time>end {if let Some(index)=self.pending.take(){self.select(index,false)} else {self.time=self.time-end+self.boundaries.get(self.index).copied().unwrap_or(0.);}}}
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) struct Flat;
    impl Environment for Flat {
        fn align(&self, old: Vec3, new: Vec3, body: &Body, state: &mut SurfaceState) -> Aligned {
            ao_formats::playfield::collision::Collision::from_scene(&ao_scene::Scene::default()).align(old.to_array(), new.to_array(), body, state)
        }
        fn ground(&self,p:Vec3)->Option<(Vec3,Vec3)> {Some((Vec3::new(p.x,0.,p.z),Vec3::Y))}
        fn ground_ray(&self,p:Vec3)->Option<(Vec3,Vec3)> {self.ground(p)}
        fn alignment(&self,_:Vec3)->Vec3 {Vec3::Y}
        fn liquid(&self,_:Vec3)->Option<f32> {None}
        fn district(&self,_:Vec3)->Option<usize> {Some(0)}
        fn obstacle(&self,_:Vec3,_:Vec3)->Option<(Vec3,Vec3)> {None}
        fn characters(&self)->&[CharacterSensor] {&[]}
        fn dangers(&self)->&[CharacterSensor] {&[]}
        fn corpses(&self)->&[Vec3] {&[]}
        fn wind(&self)->Vec3 {Vec3::ZERO}
        fn landing(&self,_:Vec3,_:f32)->Option<Vec3> {None}
    }
    #[test]
    fn native_species_share_constructor_and_steering_crt_stream() {
        let mut shared = CrtRand::thread();
        shared.reseed(123);
        let mut a = Species::new(LifeKind::Fish, 0, Vec3::Y, 0, shared.clone());
        let mut b = Species::new(LifeKind::Fish, 0, Vec3::Y, 0, shared);
        assert_eq!((a.surface.medium, b.surface.medium), (3, 3));
        let mut expected = CrtRand::new(123);
        assert_eq!(a.random(), expected.rand() as f32 / 32767.);
        assert_eq!(b.random(), expected.rand() as f32 / 32767.);
        a.rng.reseed(42); // DS1001975e reseeds the same stream through another handle.
        assert_eq!(b.random(), CrtRand::new(42).rand() as f32 / 32767.);
    }
    #[test]
    fn native_support_state_controls_gravity_and_survives_substeps() {
        let mut rabbit = Species::new(LifeKind::Rabbit, 0, Vec3::Y * 0.01, 0, ao_scene::CrtRand::new(1));
        assert_eq!(rabbit.surface.medium, 1);
        rabbit.place(&Flat);
        assert!(!rabbit.airborne);
        rabbit.tick(0.1, &Flat, &[], true, true);
        assert_eq!(rabbit.gravity_velocity, 0.);
        assert!((rabbit.pos.y - 0.01).abs() < 0.00001);
        rabbit.pos.y = 10.;
        rabbit.place(&Flat);
        assert!(rabbit.airborne);
        rabbit.tick(0.4, &Flat, &[], true, true);
        assert!((rabbit.pos.y - 6.8).abs() < 0.00001);
        assert_eq!(rabbit.gravity_velocity, -8.);
        rabbit.tick(0.4, &Flat, &[], true, true);
        rabbit.tick(0.4, &Flat, &[], true, true);
        assert!(!rabbit.airborne);
        assert_eq!(rabbit.gravity_velocity, 0.);
        let mut fish = Species::new(LifeKind::Fish, 0, Vec3::Y * 10., 0, ao_scene::CrtRand::new(1));
        fish.place(&Flat);
        assert!(!fish.airborne, "DisableFalling never starts the gravity branch");
    }
    #[test]
    fn native_lifetime_visibility_and_authored_animation_segments() {
        let mut s=Species::new(LifeKind::Beetle, 0, Vec3::ZERO, 0, ao_scene::CrtRand::new(1));
        assert_eq!((s.state,s.animation,s.max_speed,s.mass),(1,1,0.8,0.1));
        s.lifespan=1.;
        s.tick(1.,&Flat,&[],true,false);
        assert!(s.alive);
        assert_eq!(s.timer,0.); // distance-culled: no Vehicle or animation tick, lifespan still runs.
        s.tick(0.01,&Flat,&[],true,false);
        assert!(!s.alive);
        let mut s=Species::new(LifeKind::TumbleWeed, 0, Vec3::Y*100., 0, ao_scene::CrtRand::new(1));
        s.place(&Flat);
        s.tick(0.4,&Flat,&[],true,true);
        assert!((s.pos.y-96.8).abs()<0.0001);
        s.tick(0.1,&Flat,&[],false,true);
        assert!(!s.alive);
        let mut a=Animation::new(vec![0.,1.,3.].into(),4.);
        a.select(1,false);
        a.tick(2.);
        assert_eq!(a.time,3.); // native comparison is strict greater-than.
        a.tick(0.25);
        assert_eq!(a.time,1.25);
        a.select(2,true);
        a.tick(2.);
        assert_eq!((a.index,a.time),(2,3.));
    }
    #[test]
    fn native_flat_orientation_and_same_segment_restart() {
        let mut s=Species::new(LifeKind::Bird, 0, Vec3::ZERO, 0, ao_scene::CrtRand::new(1));
        s.vel=Vec3::new(1.,4.,1.);
        s.orient(&Flat);
        assert!((s.rotation*Vec3::Z).y.abs()<0.00001);
        let serial=s.animation_serial;
        s.animate(s.animation,false);
        assert_eq!(s.animation_serial,serial+1);
        s.animate(2,true);
        assert_eq!(s.animation_serial,serial+1);
    }
    #[test]
    fn native_force_budget_returns_consumed_magnitude_not_vector_length() {
        let s=Species::new(LifeKind::TumbleWeed, 0, Vec3::ZERO, 0, ao_scene::CrtRand::new(1));
        let mut force=Vec3::X*0.5;
        assert_eq!(s.accumulate(&mut force,-Vec3::X),1.);
        assert_eq!(force,Vec3::ZERO);
    }
}
