//! GC CreateGfxControlTracer 100d1218, CreateTracer 100d1e40, GetTracerSpeed 100d22cb.
//! Cylinder: GC 100fd699/100fd754/100fd855; DS 100109a4/10010472.
use super::{Template, tint, quad};
use anyhow::{ensure, Result};
use ao_scene::{Blend, Vertex, TextureKey};
use glam::{Mat4, Vec3};
use ao_formats::character::CrtRand;

pub(super) struct BeamState {
    rng: CrtRand,
    phase: [f32;3],
    phase_step: f32,
}
impl Default for BeamState {
    fn default()->Self { Self { rng:CrtRand::new(1),phase:[0.0,2.0734513,4.1469026],phase_step:0.314 } }
}

pub(super) struct Beam {
    template: Template,
    source: Vec3,
    direction: Vec3,
    perpendicular: Vec3,
    distance: f32,
    speed: f32,
    color: u32,
    previous_time: f32,
    emitted: f32,
    sprites: Vec<(f32,f32)>,
    duration: f32,
    bands: Vec<f32>,
    segment_transform: Mat4,
}

impl Beam {
    pub(super) fn supports(kind:i32)->bool {matches!(kind,1013|1019|1021|1024|1026)}
    pub(super) fn new(template: &Template, source: Mat4, target: Mat4, color: u32) -> Result<Self> {
        ensure!(Self::supports(template.kind), "unsupported beam class {}", template.kind);
        ensure!(template.words.len() >= match template.kind { 1026=>27,1019=>21,1021=>19,_=>20 }, "short beam template");
        for parameter in 8..=16 { if parameter!=9 { template.float(parameter)?; } }
        if template.kind==1021 { ensure!((2.0..=4096.0).contains(&template.float(11)?), "invalid trail band count"); }
        let source = source.w_axis.truncate();
        let delta = target.w_axis.truncate() - source;
        let distance = delta.length();
        ensure!(distance.is_finite() && distance >= 0.01, "degenerate tracer endpoints");
        let direction = delta / distance;
        let speed = (if template.kind==1019 { 100.0 } else { template.float(10)? }).min(distance * 5.0);
        ensure!(speed > 0.0 && (template.kind==1019 || (template.float(12)? > 0.0 && template.float(11)? > 0.0)), "invalid tracer dimensions");
        if template.kind == 1013 {
            ensure!(template.word(18)? <= template.word(17)? && template.word(17)? <= 7, "invalid cylinder layer range");
        }
        // GC100d2e0e runs in native coordinates; mirror the result back to scene.
        let native_direction=Vec3::new(direction.x,direction.y,-direction.z);
        let mut perpendicular = Vec3::new(native_direction.y-native_direction.z, native_direction.z-native_direction.x, native_direction.x-native_direction.y);
        if perpendicular == Vec3::ZERO {
            perpendicular = Vec3::new(native_direction.z+native_direction.y, native_direction.z-native_direction.x, -native_direction.x-native_direction.y);
        }
        perpendicular = perpendicular.normalize()*Vec3::new(1.0,1.0,-1.0);
        if template.kind == 1026 {
            ensure!(matches!(template.word(19)?,0|3), "invalid tracer return links");
            ensure!(template.float(20)? > 0.0 && template.float(21)? >= 0.0, "invalid tracer return speed");
            for parameter in 17..=26 { if parameter != 19 { template.float(parameter)?; } }
        }
        let basis=Mat4::from_cols((-direction.cross(perpendicular)).extend(0.0),perpendicular.extend(0.0),(-direction).extend(0.0),source.extend(1.0));
        let segment_transform=if template.kind==1019 {
            let segments=template.word(19)? as usize;
            ensure!((template.word(9)? as usize)<super::materials::MATERIALS.len(),"invalid segmented tracer material");
            ensure!(template.float(18)? > 0.0,"invalid segmented tracer width");
            let copies=template.word(20)? as usize;
            ensure!(segments>0 && copies>0 && segments.checked_mul(copies).is_some_and(|n| n<=4096), "invalid segmented tracer count");
            ensure!(template.words.len()>=21+segments*6,"short segmented tracer endpoints");
            for i in 21..21+segments*6 { template.float(i)?; }
            super::sprites::connector(template,basis)?
        } else { basis };
        Ok(Self { template: template.clone(), source, direction, perpendicular, distance, speed, color, previous_time:0.0,emitted:0.0,sprites:Vec::new(),duration:template.float(8)?,bands:vec![0.0],segment_transform })
    }

    pub(super) fn set_duration(&mut self,duration:f32) { self.duration=duration; }

    // GC10105ff9: explicit TYPE2 connector replacement, not dynel tracking.
    pub(super) fn update_source(&mut self,source:Mat4)->Result<()> {
        ensure!(self.template.kind==1019,"beam class {} has no authored connector source setter",self.template.kind);
        self.segment_transform=super::sprites::connector(&self.template,source)?;
        Ok(())
    }

    pub(super) fn texture_override(&self,_group:usize)->Option<TextureKey> {
        (self.template.kind==1021).then_some(TextureKey { rdb_type:1010004,id:8406 })
    }

    fn layers(&self) -> impl Iterator<Item=u32> { (self.template.words[18]..=self.template.words[17]).rev() }

    pub(super) fn models(&self) -> Vec<(Option<usize>, Vec<u32>, usize)> {
        if self.template.kind==1019 {
            let count=self.template.words[19]*self.template.words[20];
            return vec![(Some(self.template.words[9] as usize),(0..count).flat_map(|i| {let a=i*4;[a,a+2,a+3,a,a+3,a+1]}).collect(),count as usize*4)];
        }
        if self.template.kind==1021 {
            let links=f32::from_bits(self.template.words[11]) as u32;
            return vec![(None,ribbon_indices(links,2),(links*4) as usize)];
        }
        if self.template.kind == 1026 {
            let links=self.template.words[19];
            let quads=(0..64u32).flat_map(|i| { let a=i*4; [a,a+2,a+3,a,a+3,a+1] }).collect();
            return vec![(Some(15),ribbon_indices(links,3),(links*6) as usize),(Some(self.template.words[9] as usize),quads,256)];
        }
        if self.template.kind == 1024 {
            return vec![(Some(self.template.words[9] as usize), ribbon_indices(20,3),120)];
        }
        self.layers().map(|_| {
            let mut indices = Vec::with_capacity(16*12);
            for i in 0..16u32 {
                let a = i*2;
                indices.extend_from_slice(&[a,a+1,a+2,a+2,a+1,a+3]);
                if self.template.words[19] == 0 { indices.extend_from_slice(&[34,35+i,36+i]); }
                indices.extend_from_slice(&[52,53+i,54+i]);
            }
            let material = (self.template.words[9] != u32::MAX).then_some(self.template.words[9] as usize);
            (material, indices, 70)
        }).collect()
    }

    pub(super) fn blends(&self) -> Vec<Blend> {
        if self.template.kind==1019 { return vec![Blend::Additive]; }
        if self.template.kind==1021 { return vec![Blend::Additive]; }
        if self.template.kind == 1026 { return vec![Blend::Additive;2]; }
        if self.template.kind == 1024 { return vec![Blend::Additive]; }
        self.layers().map(|_| Blend::PremultipliedAlpha).collect()
    }

    pub(super) fn vertices(&mut self, time: f32, _camera: Vec3, _right: Vec3, _up: Vec3, state:&mut BeamState) -> Result<Option<Vec<Vec<Vertex>>>> {
        ensure!(time.is_finite() && time >= 0.0, "invalid tracer time");
        if self.template.kind == 1024 { return self.cords(time,_camera,state); }
        if self.template.kind == 1026 { return self.return_tracer(time,_camera,_right,_up); }
        if self.template.kind==1021 { return self.trail(time); }
        if self.template.kind==1019 { return self.segments(time,_camera,_right,_up); }
        let tail = self.speed*time;
        if tail > self.distance || (self.duration>0.0 && time>self.duration) { return Ok(None); }
        let head = (tail+self.template.float(11)?).min(self.distance);
        let radius = self.template.float(12)?;
        let x = self.direction.cross(self.perpendicular).normalize();
        let z = self.perpendicular;
        let native: [u8;3] = if self.color==0 {
            [14,15,16].map(|i| (f32::from_bits(self.template.words[i])*255.0).clamp(0.0,255.0) as u8)
        } else { let [_,r,g,b]=self.color.to_be_bytes(); [r,g,b] };
        let groups = self.layers().map(|layer| {
            // DS 10010472: layers 0..5 = radius*(8..3)/8, 6=radius/4, 7=radius/8.
            let scale = match layer { 0..=5 => (8-layer) as f32/8.0, 6 => 0.25, _ => 0.125 };
            let r = radius*scale;
            let lo = tail+radius-r;
            let hi = head-radius+r;
            let alpha = if layer == 7 { 128.0/255.0 } else { 0.0 };
            // DS shifts packed gamma RGB before transfer to the linear render target.
            let rgb=native.map(|channel| { let channel=if layer<6 { channel>>2 } else { channel }; (channel as f32/255.0).powf(2.2) });
            let color=[rgb[0],rgb[1],rgb[2],alpha];
            let vertex = |position: Vec3, uv: [f32;2], color: [f32;4]| Vertex { pos: position.to_array(), normal: self.direction.to_array(), uv, color };
            let mut vertices = Vec::with_capacity(70);
            for i in 0..=16 {
                let angle = std::f32::consts::TAU*i as f32/16.0;
                let radial = (x*angle.cos()+z*angle.sin())*r;
                let mut back_color = color;
                if self.template.words[19] != 0 { back_color = [0.0;4]; }
                vertices.push(vertex(self.source+self.direction*lo+radial,[i as f32/16.0,1.0],back_color));
                vertices.push(vertex(self.source+self.direction*hi+radial,[i as f32/16.0,0.0],color));
            }
            for (end,reverse) in [(lo,false),(hi,true)] {
                vertices.push(vertex(self.source+self.direction*end,[0.0;2],color));
                for i in 0..=16 {
                    let index = if reverse { 16-i } else { i };
                    let angle = std::f32::consts::TAU*index as f32/16.0;
                    vertices.push(vertex(self.source+self.direction*end+(x*angle.cos()+z*angle.sin())*r,[0.0;2],color));
                }
            }
            vertices
        }).collect();
        Ok(Some(groups))
    }
    // GC100fdfef,100fe1b0: authored endpoint pairs rotated around local Y,
    // rendered as native FlareType0 sprites, not a replacement ribbon.
    fn segments(&self,time:f32,camera:Vec3,right:Vec3,up:Vec3)->Result<Option<Vec<Vec<Vertex>>>> {
        if time*self.speed>self.distance || (self.duration>0.0 && time>self.duration) { return Ok(None); }
        let (_,columns,rows,first,_)=super::materials::MATERIALS[self.template.word(9)? as usize];
        let (u,v)=(first%columns,first/rows);
        let uv=[[u as f32/columns as f32,(v+1) as f32/rows as f32],[u as f32/columns as f32,v as f32/rows as f32],[(u+1) as f32/columns as f32,(v+1) as f32/rows as f32],[(u+1) as f32/columns as f32,v as f32/rows as f32]];
        // Native NewSprite life=1 and stop ARGB=0 (GC100fdfef).
        let fade=(1.0-time).max(0.0);
        let color=[(self.template.float(11)?*fade).max(0.0).powf(2.2),(self.template.float(12)?*fade).max(0.0).powf(2.2),(self.template.float(13)?*fade).max(0.0).powf(2.2),self.template.float(10)?*fade];
        let width=self.template.float(18)?;
        let mut vertices=Vec::with_capacity((self.template.words[19]*self.template.words[20]*4) as usize);
        for copy in 0..self.template.words[20] {
            let angle=copy as f32/self.template.words[20] as f32*6.2831855;
            let (sin,cos)=angle.sin_cos();
            let endpoint=|i:usize| {
                let x=f32::from_bits(self.template.words[i]);
                let y=f32::from_bits(self.template.words[i+1]);
                let z=f32::from_bits(self.template.words[i+2]);
                self.segment_transform.transform_point3(Vec3::new(x*cos-z*sin,y,-(x*sin+z*cos)))+self.direction*self.speed*time
            };
            for segment in 0..self.template.words[19] {
                let p=endpoint(21+segment as usize*6);
                let q=endpoint(24+segment as usize*6);
                let forward=right.cross(up);
                let view=|p:Vec3| {let d=p-camera;glam::Vec2::new(d.dot(right),d.dot(up))/d.dot(forward)};
                let d=(view(q)-view(p)).normalize_or_zero();
                let (a,b)=if d==glam::Vec2::ZERO {(right*width,up*width)} else {((right*d.x+up*d.y)*width,(right*d.y-up*d.x)*width)};
                quad(&mut vertices,[p-a-b,p-a+b,q+a-b,q+a+b],uv,color);
            }
        }
        Ok(Some(vec![vertices]))
    }

    // GC100feacd / DS10031830: two orthogonal native PathBlur strips.
    fn trail(&mut self,time:f32)->Result<Option<Vec<Vec<Vertex>>>> {
        if self.duration>0.0 && time>self.duration { return Ok(None); }
        let dt=(time-self.previous_time).max(0.0);
        self.previous_time=time;
        if self.duration==-1.0 {
            self.emitted+=self.speed*dt;
            if self.emitted>self.distance { return Ok(None); }
        }
        let count=self.template.float(11)? as usize;
        self.bands.insert(0,self.emitted);
        self.bands.truncate(count);
        let radius=self.template.float(12)?;
        let mut vertices=Vec::with_capacity(count*4);
        let color=tint(&self.template,self.color,0.0,13);
        for side in [self.perpendicular,self.direction.cross(self.perpendicular).normalize()] {
            for i in 0..count {
                let fraction=i as f32/(count-1) as f32;
                let mut color=color;
                color[3]*=1.0-fraction;
                if i>=self.bands.len() { color=[0.0;4]; }
                let center=self.source+self.direction*self.bands.get(i).copied().unwrap_or(self.emitted);
                for (position,v) in [(center+side*radius,0.0),(center-side*radius,1.0)] {
                    vertices.push(Vertex { pos:position.to_array(),normal:self.direction.to_array(),uv:[fraction,v],color });
                }
            }
        }
        Ok(Some(vec![vertices]))
    }

    // GC 100ff811/100ff525: three independently phased, twenty-link cords.
    fn cords(&mut self, time:f32, camera:Vec3, state:&mut BeamState) -> Result<Option<Vec<Vec<Vertex>>>> {
        if self.duration>0.0 && time>self.duration { return Ok(None); }
        let dt = (time-self.previous_time).max(0.0);
        self.previous_time=time;
        let width=self.template.float(11)?;
        let radius=self.template.float(12)?;
        let x=self.direction.cross(self.perpendicular).normalize();
        let mut vertices=Vec::with_capacity(120);
        let color=tint(&self.template,self.color,0.0,13);
        for cord in 0..3 {
            state.phase[cord]+=dt*12.56;
            if state.rng.rand() & 3 == 0 { state.phase_step = -state.phase_step; }
            let mut positions=[Vec3::ZERO;20];
            let mut phase=state.phase[cord];
            for (i,position) in positions.iter_mut().enumerate() {
                let fraction=i as f32*0.05;
                let r=((fraction*std::f32::consts::PI*20.0/19.0).sin()+0.2)*radius;
                *position=self.source+self.direction*(self.distance*1.1*fraction)+(x*phase.cos()+self.perpendicular*phase.sin())*r;
                if state.rng.rand() & 3 == 0 {
                    let jitter_x=(state.rng.rand() as f32/32767.0*2.0-1.0)*r*0.5;
                    let jitter_z=(state.rng.rand() as f32/32767.0*2.0-1.0)*r*0.5;
                    *position+=x*jitter_x+self.perpendicular*jitter_z;
                }
                phase+=state.phase_step;
            }
            ribbon(&mut vertices,&positions,width,camera,color);
        }
        Ok(Some(vec![vertices]))
    }

    // GC 10100855/10100b27: outward sprite trail and three returning cords.
    fn return_tracer(&mut self,time:f32,camera:Vec3,_right:Vec3,_up:Vec3)->Result<Option<Vec<Vec<Vertex>>>> {
        let t=&self.template;
        if self.duration>0.0 && time>self.duration { return Ok(None); }
        let dt=(time-self.previous_time).max(0.0);
        self.previous_time=time;
        let spacing=t.float(11)?.min(self.distance*0.24);
        let outward_head=(self.speed*time+spacing).min(self.distance);
        if time>self.duration && self.speed*time>self.distance { self.duration=time+1.0; }
        for sprite in &mut self.sprites { sprite.1+=dt; }
        self.sprites.retain(|sprite| sprite.1<1.0);
        if time > self.duration {
            while self.emitted<outward_head {
                // DS 100288f3: full 64-slot pool returns null, it never evicts a live sprite.
                if self.sprites.len()<64 { self.sprites.push((self.emitted,0.0)); }
                self.emitted+=spacing;
            }
        }
        let return_speed=t.float(20)?;
        let return_time=(time-(self.distance/self.speed-self.distance/return_speed)).max(0.0);
        let tail=return_speed*return_time;
        let head=(tail+t.float(21)?).min(self.distance);
        if time > self.duration && tail>self.distance { self.duration=time+1.0; }
        let mut cords=Vec::with_capacity(t.words[19] as usize*6);
        let mut cord_color=tint(t,self.color,0.0,23);
        if time<self.duration { cord_color=[0.0;4]; }
        // Native allocates arbitrary link count but updates only the first three.
        for _ in 0..if t.words[19]==0 { 0 } else { 3 } {
            let mut points=vec![self.source;t.words[19] as usize];
            if time>=self.duration && return_time>0.0 {
                points[0]=self.source+self.direction*head;
                points[1]=self.source+self.direction*tail.min(self.distance);
            }
            ribbon(&mut cords,&points,t.float(22)?,camera,cord_color);
        }
        let mut sprites=Vec::with_capacity(256);
        // DS 10028b9c: Sprite3 uses authored local axes, not camera billboards.
        let right=self.direction.cross(self.perpendicular).normalize();
        let up=self.perpendicular;
        let color=tint(t,self.color,0.0,13);
        for &(position,age) in &self.sprites {
            let size=t.float(12)?+position*t.float(17)?+age*t.float(18)?;
            let center=self.source+self.direction*position;
            // x87 10100c20..10100c49: frame = trunc(age*30+33).
            let frame=(age*30.0+33.0) as u32;
            let (u,v)=((frame%8) as f32/8.0,(frame/8) as f32/8.0);
            let mut color=color; color[3]=1.0-age;
            let half=size*0.5;
            quad(&mut sprites,[center-right*half-up*half,center-right*half+up*half,center+right*half-up*half,center+right*half+up*half],[[u,v+0.125],[u,v],[u+0.125,v+0.125],[u+0.125,v]],color);
        }
        sprites.resize(256,Vertex::default());
        Ok(Some(vec![cords,sprites]))
    }
}

fn ribbon_indices(links:u32,cords:u32)->Vec<u32> {
    if links<2 { return Vec::new(); }
    (0..cords).flat_map(|cord| (0..links-1).flat_map(move |i| {
        let a=(cord*links+i)*2;
        [a,a+1,a+2,a+1,a+2,a+3]
    })).collect()
}

// DS 1000e3e8: shared two-vertex links, averaged side at interior joins.
fn ribbon(vertices:&mut Vec<Vertex>,positions:&[Vec3],width:f32,camera:Vec3,color:[f32;4]) {
    let side_at=|i:usize| (positions[i+1]-positions[i]).cross(camera-positions[i]).normalize_or_zero();
    for (i,&p) in positions.iter().enumerate() {
        let side=if i==0 { side_at(0) } else if i==positions.len()-1 { side_at(i-1) } else { (side_at(i-1)+side_at(i))*0.5 };
        for (position,u) in [(p-side*width,0.0),(p+side*width,1.0)] {
            vertices.push(Vertex { pos:position.to_array(),normal:[0.0,0.0,1.0],uv:[u,1.0],color });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_pistol_tracer_has_retail_cylinder_layers_and_endpoint_lifetime() {
        let mut words = vec![0;20];
        words[8]=20f32.to_bits(); words[9]=u32::MAX; words[10]=75f32.to_bits();
        words[11]=5f32.to_bits(); words[12]=0.075f32.to_bits();
        words[14]=1f32.to_bits(); words[15]=1f32.to_bits(); words[17]=7; words[18]=6; words[19]=1;
        let template = Template { kind:1013, words };
        let mut beam = Beam::new(&template,Mat4::IDENTITY,Mat4::from_translation(Vec3::Y*10.0),0).unwrap();
        let models = beam.models();
        assert_eq!(models.len(),2);
        assert_eq!(beam.blends(),vec![Blend::PremultipliedAlpha;2]);
        assert!(models.iter().all(|(material,indices,count)| material.is_none() && *count==70 && indices.iter().all(|i| *i<70)));
        let frame = beam.vertices(0.0,Vec3::ZERO,Vec3::X,Vec3::Y,&mut BeamState::default()).unwrap().unwrap();
        assert_eq!(frame.iter().map(Vec::len).collect::<Vec<_>>(),vec![70,70]);
        assert!(frame.iter().flatten().all(|v| v.pos.iter().all(|f| f.is_finite())));
        assert!(beam.vertices(0.201,Vec3::ZERO,Vec3::X,Vec3::Y,&mut BeamState::default()).unwrap().is_none());
    }
    #[test]
    fn cord_and_return_tracer_groups_match_authored_visual_pools() {
        for kind in [1024,1026] {
            let mut words=vec![0;27];
            words[8]=0.1f32.to_bits(); words[9]=15;
            words[10]=25f32.to_bits(); words[11]=1f32.to_bits(); words[12]=0.1f32.to_bits();
            words[13]=1f32.to_bits(); words[14]=1f32.to_bits(); words[15]=1f32.to_bits();
            words[19]=3; words[20]=25f32.to_bits(); words[21]=1f32.to_bits(); words[22]=0.1f32.to_bits();
            words[23]=1f32.to_bits(); words[24]=1f32.to_bits();
            let mut beam=Beam::new(&Template { kind,words },Mat4::IDENTITY,Mat4::from_translation(Vec3::Y*10.0),0).unwrap();
            let models=beam.models();
            let frames=beam.vertices(0.05,Vec3::Z*10.0,Vec3::X,Vec3::Y,&mut BeamState::default()).unwrap().unwrap();
            assert_eq!(frames.len(),models.len());
            for (frame,(_,indices,count)) in frames.iter().zip(&models) {
                assert_eq!(frame.len(),*count);
                assert!(indices.iter().all(|index| (*index as usize)<*count));
            }
        }
    }
    #[test]
    fn segmented_tracer_uses_authored_pairs_and_native_default_speed() {
        let mut words=vec![0;27];
        words[8]=20f32.to_bits(); words[9]=15;
        words[10]=0.5f32.to_bits(); words[11]=1f32.to_bits(); words[12]=1f32.to_bits();
        words[13]=1f32.to_bits(); words[18]=0.1f32.to_bits(); words[19]=1; words[20]=3;
        words[24]=1f32.to_bits();
        let mut beam=Beam::new(&Template {kind:1019,words},Mat4::IDENTITY,Mat4::from_translation(Vec3::Z*10.0),0).unwrap();
        assert_eq!(beam.speed,50.0);
        assert_eq!(beam.models()[0].2,12);
        let frame=beam.vertices(0.05,Vec3::new(0.0,0.0,-10.0),Vec3::X,Vec3::Y,&mut BeamState::default()).unwrap().unwrap();
        assert_eq!(frame[0].len(),12);
        assert!(frame[0].iter().all(|v|v.pos.iter().all(|f|f.is_finite())));
        beam.update_source(Mat4::from_translation(Vec3::X*3.0)).unwrap();
        assert_eq!(beam.segment_transform.w_axis.truncate(),Vec3::X*3.0);
        assert_eq!(beam.speed,50.0);
    }
    #[test]
    fn trail_uses_native_texture_and_two_fixed_band_strips() {
        let mut words=vec![0;19];
        words[8]=(-1f32).to_bits();words[9]=u32::MAX;words[10]=100f32.to_bits();
        words[11]=8f32.to_bits();words[12]=0.25f32.to_bits();
        words[13]=1f32.to_bits();words[14]=1f32.to_bits();words[15]=1f32.to_bits();words[16]=1f32.to_bits();
        let mut beam=Beam::new(&Template {kind:1021,words},Mat4::IDENTITY,Mat4::from_translation(Vec3::Z*10.0),0).unwrap();
        assert_eq!(beam.texture_override(0),Some(TextureKey {rdb_type:1010004,id:8406}));
        assert_eq!(beam.models()[0].2,32);
        let frame=beam.vertices(0.01,Vec3::X,Vec3::X,Vec3::Y,&mut BeamState::default()).unwrap().unwrap();
        assert_eq!(frame[0].len(),32);
        assert!(frame[0].iter().all(|v|v.pos.iter().all(|f|f.is_finite())));
    }
}
