//! Native `_GfxHitLocation_t` registry: NewHitLocation GC100cda79/101056b4,
//! ctor10104dca, getters10104fa8/10104fde, connector overrides101050a4/101050ba,
//! registry retention10105621 and locator invalidation1010603b.
//! Positions/matrices are already reflected scene coordinates (`F*M*F`).
use glam::{Mat4, Vec3};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HitLocationRequest {
    pub source: (u32, u32),
    pub target: (u32, u32),
    pub source_attractor: i32,
    pub target_attractor: i32,
    /// Native NewHitLocation bool: hits follow the endpoint, misses lock it.
    pub hit: bool,
}

#[derive(Clone, Copy)]
struct Locator {
    identity: (u32,u32),
    attractor: i32,
    matrix: Mat4,
    active: bool,
}
impl Locator {
    fn sample(&mut self, resolve: &mut impl FnMut((u32,u32),i32)->Option<Mat4>) -> Mat4 {
        if self.active {
            if let Some(matrix) = resolve(self.identity,self.attractor) {
                self.matrix = matrix;
            } else {
                // GC1010603b sets locator type0 permanently. 10105eb4 still
                // returns the last position: no invented origin or reattachment.
                self.active = false;
            }
        }
        self.matrix
    }
}

pub(super) struct HitLocation {
    pub born_ms: u64,
    source: Locator,
    target: Locator,
    hit: bool,
    start: Vec3,
    end: Vec3,
    locked_end: Option<Vec3>,
}
impl HitLocation {
    fn new(born_ms:u64, request:HitLocationRequest, source:Mat4, target:Mat4) -> Self {
        Self { born_ms, source:Locator {identity:request.source,attractor:request.source_attractor,matrix:source,active:true},
            target:Locator {identity:request.target,attractor:request.target_attractor,matrix:target,active:true}, hit:request.hit,
            start:source.w_axis.truncate(),end:target.w_axis.truncate(),locked_end:None }
    }
    pub fn request(&self) -> HitLocationRequest {
        HitLocationRequest {source:self.source.identity,target:self.target.identity,
            source_attractor:self.source.attractor,target_attractor:self.target.attractor,hit:self.hit}
    }
    /// GC101050a4 /101050ba update both the hit metadata and shared locator.
    /// They deliberately do not unlock an already locked miss endpoint.
    pub fn set_source_attractor(&mut self, attractor:i32) { self.source.attractor=attractor; }
    pub fn set_target_attractor(&mut self, attractor:i32) { self.target.attractor=attractor; }
    pub fn sample_start(&mut self, resolve:&mut impl FnMut((u32,u32),i32)->Option<Mat4>) -> Vec3 {
        self.start=self.source.sample(resolve).w_axis.truncate();
        self.start
    }
    pub fn sample_end(&mut self, resolve:&mut impl FnMut((u32,u32),i32)->Option<Mat4>) -> Vec3 {
        let matrix=self.target.sample(resolve);
        self.end=matrix.w_axis.truncate();
        if !self.hit && self.locked_end.is_none() && self.end-self.start != Vec3::ZERO {
            // GC10104fde +1015f168: endpoint - native E2*10. In scene
            // coordinates native E2 is -matrix.z_axis; keep its authored scale.
            self.locked_end=Some(self.end+matrix.z_axis.truncate()*10.0);
        }
        if let Some(locked)=self.locked_end { self.end=locked; }
        self.end
    }
    pub fn sample(&mut self, resolve:&mut impl FnMut((u32,u32),i32)->Option<Mat4>) -> (Vec3,Vec3) {
        let start=self.sample_start(resolve);
        let end=self.sample_end(resolve);
        (start,end)
    }
    #[cfg(test)]
    pub fn locator_validity(&self) -> (bool,bool) { (self.source.active,self.target.active) }
}

#[derive(Default)]
pub(super) struct HitLocations {
    locations:BTreeMap<u32,HitLocation>,
    newest:u32,
}
impl HitLocations {
    /// Requires the actual source/target connectors; never synthesizes an origin.
    pub fn create(&mut self, now_ms:u64, request:HitLocationRequest,
        resolve:&mut impl FnMut((u32,u32),i32)->Option<Mat4>) -> Option<u32> {
        let source=resolve(request.source,request.source_attractor)?;
        let target=resolve(request.target,request.target_attractor)?;
        self.newest=self.newest.wrapping_add(1);
        self.locations.insert(self.newest,HitLocation::new(now_ms,request,source,target));
        Some(self.newest)
    }
    pub fn get_mut(&mut self, handle:u32) -> Option<&mut HitLocation> { self.locations.get_mut(&handle) }
    #[cfg(test)]
    pub fn get(&self, handle:u32) -> Option<&HitLocation> { self.locations.get(&handle) }
    pub fn requests(&self) -> impl Iterator<Item=HitLocationRequest> + '_ {
        self.locations.values().map(HitLocation::request)
    }
    pub fn source_deleted(&mut self, identity:(u32,u32)) {
        for location in self.locations.values_mut() {
            if location.source.identity==identity { location.source.active=false; }
            if location.target.identity==identity { location.target.active=false; }
        }
    }
    pub fn sample(&mut self, handle:u32, resolve:&mut impl FnMut((u32,u32),i32)->Option<Mat4>) -> Option<(Vec3,Vec3)> {
        Some(self.locations.get_mut(&handle)?.sample(resolve))
    }
    /// GC10105621 stops at the first unexpired entry and never removes newest.
    /// DeleteEffect does not delete its hit location; stale handles expire here.
    pub fn prune(&mut self, now_ms:u64) {
        while let Some((&handle,location))=self.locations.first_key_value() {
            if handle==self.newest || now_ms <= location.born_ms.saturating_add(10_000) { break; }
            self.locations.remove(&handle);
        }
    }
    pub fn clear(&mut self) { self.locations.clear(); }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(hit:bool)->HitLocationRequest {
        HitLocationRequest {source:(50000,1),target:(50000,2),source_attractor:2000,target_attractor:1000,hit}
    }
    #[test]
    fn native_miss_locks_target_forward_and_shared_connector_overrides() {
        let mut registry=HitLocations::default();
        let mut source=Mat4::from_translation(Vec3::X);
        let mut target=Mat4::from_translation(Vec3::X*4.0);
        let handle=registry.create(0,request(false),&mut |identity,_| Some(if identity.1==1 {source}else{target})).unwrap();
        let first=registry.sample(handle,&mut |identity,_| Some(if identity.1==1 {source}else{target})).unwrap();
        assert_eq!(first,(Vec3::X,Vec3::X*4.0+Vec3::Z*10.0));
        source=Mat4::from_translation(Vec3::Y);
        target=Mat4::from_translation(Vec3::X*20.0);
        registry.get_mut(handle).unwrap().set_source_attractor(3001);
        registry.get_mut(handle).unwrap().set_target_attractor(1002);
        let next=registry.sample(handle,&mut |identity,attractor| {
            assert_eq!(attractor,if identity.1==1 {3001}else{1002});
            Some(if identity.1==1 {source}else{target})
        }).unwrap();
        assert_eq!(next,(Vec3::Y,first.1));
        assert_eq!(registry.get(handle).unwrap().request().target_attractor,1002);
    }
    #[test]
    fn native_hits_track_and_missing_dynel_freezes_locator_until_registry_expiry() {
        let mut registry=HitLocations::default();
        let handle=registry.create(100,request(true),&mut |identity,_|Some(Mat4::from_translation(Vec3::X*identity.1 as f32))).unwrap();
        assert_eq!(registry.sample(handle,&mut |identity,_|Some(Mat4::from_translation(Vec3::Y*identity.1 as f32))).unwrap(),(Vec3::Y,Vec3::Y*2.0));
        assert_eq!(registry.sample(handle,&mut |_,_|None).unwrap(),(Vec3::Y,Vec3::Y*2.0));
        assert_eq!(registry.get(handle).unwrap().locator_validity(),(false,false));
        assert_eq!(registry.sample(handle,&mut |_,_|panic!("native type0 locator must not reattach")).unwrap(),(Vec3::Y,Vec3::Y*2.0));
        registry.prune(50_000);
        assert!(registry.get(handle).is_some(),"native newest entry is retained");
        let newer=registry.create(50_000,request(true),&mut |_,_|Some(Mat4::IDENTITY)).unwrap();
        registry.prune(50_000);
        assert!(registry.get(handle).is_none());
        assert!(registry.get(newer).is_some());
        registry.clear();
        assert!(registry.get(newer).is_none());
    }
    #[test]
    fn native_miss_zero_delta_does_not_lock_and_retention_boundary_is_strict() {
        let mut registry=HitLocations::default();
        assert!(registry.create(0,request(true),&mut |_,_|None).is_none());
        let handle=registry.create(200,request(false),&mut |_,_|Some(Mat4::IDENTITY)).unwrap();
        assert_eq!(registry.sample(handle,&mut |_,_|Some(Mat4::IDENTITY)).unwrap(),(Vec3::ZERO,Vec3::ZERO));
        let (_,end)=registry.sample(handle,&mut |identity,_|Some(Mat4::from_translation(if identity.1==2 {Vec3::X}else{Vec3::ZERO}))).unwrap();
        assert_eq!(end,Vec3::X+Vec3::Z*10.0);
        registry.create(201,request(true),&mut |_,_|Some(Mat4::IDENTITY)).unwrap();
        registry.prune(10_200);
        assert!(registry.get(handle).is_some());
        registry.prune(10_201);
        assert!(registry.get(handle).is_none());
    }
}
