//! VisualWater geometry: DS1003a3b4/10039f14/1003e387/1003e94f/1003f4ee.
use crate::Vertex;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct SourceTriangle {
    pub kind: u32,
    /// Scene coordinates (+Y up, AO Z mirrored); UV uses the native liquid scale.
    pub vertices: [Vertex; 3],
}

#[derive(Clone, Copy, Debug)]
struct Point { vertex: Vertex, argb: u32 }
#[derive(Clone, Debug)]
struct Triangle { indices:[u32;3], cell:i32 }

#[derive(Clone, Debug)]
pub struct Surface {
    pub mesh: usize,
    /// Native draw style: water0, lava1, acid2, star3, shallow4.
    pub style: u32,
    vertices: Arc<[Point]>,
    triangles: Arc<[Triangle]>,
}

pub fn style(kind: u32) -> Option<u32> {
    match kind {
        0|1|4|5|8|9|10|11|12|13 => Some(0),
        2|3 => Some(1), 6|7 => Some(2), 14|15 => Some(3),
        16|17 => None, 18|19 => Some(4), _ => Some(0),
    }
}

impl Surface {
    pub fn new(mesh: usize, style: u32, source: Vec<SourceTriangle>) -> Result<Self, &'static str> {
        if style > 4 { return Err("invalid native liquid style"); }
        let mut vertices=Vec::new();let mut triangles=Vec::new();
        for source in source {
            if self::style(source.kind)!=Some(style) { return Err("liquid kind/style mismatch"); }
            if source.vertices.iter().any(|v|v.pos.iter().chain(&v.uv).any(|x|!x.is_finite())) {return Err("nonfinite liquid geometry");}
            let mut triangle=source.vertices.map(|vertex|Point {vertex,argb:argb(source.kind)});
            let mut normal=cross(sub(triangle[1].vertex.pos,triangle[0].vertex.pos),sub(triangle[2].vertex.pos,triangle[0].vertex.pos));
            if normal.iter().any(|v|!v.is_finite()) {return Err("liquid geometry overflow");}
            if normal[1]<0.0 {triangle.swap(1,2);normal=normal.map(|v|-v);}
            if normal.iter().all(|v|v.abs()<0.00001) {continue;}
            let length=length(normal);
            if !length.is_finite() {return Err("liquid normal overflow");}
            normal=normal.map(|v|v/length);
            for point in &mut triangle {point.vertex.normal=normal;}
            initial(triangle,source.kind&1==0,0,&mut vertices,&mut triangles)?;
        }
        // DS1003f4ee iterates column first, then row; vertex indices drive its wave table.
        triangles.sort_by_key(|triangle|(triangle.cell%60)*60+triangle.cell/60);
        // Validate a conservative all-cells fine bound once; render uses reusable u32 buffers.
        let mut bound=0u64;
        for triangle in &triangles {
            let points=triangle.indices.map(|i|vertices[i as usize]);
            let mut area=magnitude(points);
            let mut edge=(0..3).map(|i|length(sub(points[i].vertex.pos,points[(i+1)%3].vertex.pos))).fold(0.0f32,f32::max);
            if !area.is_finite()||!edge.is_finite() {return Err("liquid refinement overflow");}
            let mut depth=0;
            while area>110.0||edge>20.0 {
                if depth>=23 {return Err("liquid refinement depth limit");}
                depth+=1;area*=0.25;edge*=0.5;
            }
            // One extra level covers float rounding at a split threshold.
            bound=bound.checked_add(3u64<<(2*(depth+1))).ok_or("liquid refinement vertex overflow")?;
            if bound>u64::from(u32::MAX) {return Err("liquid refinement vertex limit");}
        }
        Ok(Self {mesh,style,vertices:vertices.into(),triangles:triangles.into()})
    }
    /// A source centroid for initial GPU material upload; the first draw uses the real camera.
    pub fn initial_camera(&self)->Option<[f32;3]> {
        let triangle=self.triangles.first()?.indices.map(|i|self.vertices[i as usize].vertex.pos);
        Some(std::array::from_fn(|i|((f64::from(triangle[0][i])+f64::from(triangle[1][i])+f64::from(triangle[2][i]))/3.0) as f32))
    }

    /// Conservative source-triangle bounds exclude vegetation beside or below liquid.
    /// False positives at shoreline triangle corners only remove optional grass.
    pub fn overlaps_ground_cell(&self, x: f32, z: f32, size: f32, ground_y: f32) -> bool {
        self.triangles.iter().any(|triangle| {
            let points = triangle.indices.map(|i| self.vertices[i as usize].vertex.pos);
            let min_x = points.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min);
            let max_x = points.iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max);
            let min_z = points.iter().map(|p| p[2]).fold(f32::INFINITY, f32::min);
            let max_z = points.iter().map(|p| p[2]).fold(f32::NEG_INFINITY, f32::max);
            let max_y = points.iter().map(|p| p[1]).fold(f32::NEG_INFINITY, f32::max);
            max_y >= ground_y && max_x >= x && min_x <= x + size && max_z >= z - size && min_z <= z
        })
    }

    /// DS1003f50f..1003f533: _ftol(z/-100), _ftol(x/100), 60 cells/row.
    pub fn cell(camera: [f32;3]) -> [i32;2] {[(f64::from(camera[0])/100.0).trunc() as i32,(-f64::from(camera[2])/100.0).trunc() as i32]}

    /// DS1003f4ee: recognition-ID FX41 spheres are replaced on cell rebuild,
    /// one per original lava triangle in the near flat-index 3x3 neighbourhood.
    pub fn heat_spheres(&self, camera: [f32; 3], realistic: bool, out: &mut Vec<[f32; 4]>) {
        out.clear();
        if self.style != 1 || !realistic { return; }
        let cell = Self::cell(camera);
        let id = i64::from(cell[0]) + i64::from(cell[1]) * 60;
        for triangle in self.triangles.iter().filter(|triangle| [-61, -60, -59, -1, 0, 1, 59, 60, 61].contains(&(i64::from(triangle.cell) - id))) {
            let points = triangle.indices.map(|index| self.vertices[index as usize].vertex.pos);
            let mut center = std::array::from_fn(|axis| (points[0][axis] + points[1][axis] + points[2][axis]) / 3.0);
            let radius = points.iter().map(|point| length(sub(*point, center))).fold(0.0f32, f32::max);
            center[1] -= radius * 0.25;
            out.push([center[0], center[1], center[2], radius]);
        }
    }

    /// Rebuild only when camera cell or RealisticWater changes. Scratch capacities are retained.
    pub fn geometry(&self,camera:[f32;3],realistic:bool,vertices:&mut Vec<Vertex>,indices:&mut Vec<u32>,colors:&mut Vec<u32>) {
        vertices.clear();indices.clear();colors.clear();
        let camera=Self::cell(camera);
        let camera_id=i64::from(camera[0])+i64::from(camera[1])*60;
        let x=camera_id%60;let row=camera_id/60;
        for triangle in self.triangles.iter() {
            let id=i64::from(triangle.cell);
            if !(0..3600).contains(&id) {continue;}
            if id%60<(x-10).max(0)||id%60>(x+10).min(59)||id/60<(row-10).max(0)||id/60>(row+10).min(59) {continue;}
            let points=triangle.indices.map(|i|self.vertices[i as usize]);
            // Native camera neighbourhood is a flat-index 3x3 test, including row-edge wrap.
            let near=[-61,-60,-59,-1,0,1,59,60,61].contains(&(id-camera_id));
            refine(points,realistic&&near&&!matches!(self.style,1|3),vertices,indices,colors);
        }
        for (vertex,argb) in vertices.iter_mut().zip(colors.iter_mut()) {
            *argb=(*argb&0x00ffffff)|(u32::from(normal_alpha((*argb>>24) as u8,vertex.normal[1]))<<24);
            vertex.color=rgba(*argb);
        }
    }
}

fn argb(kind:u32)->u32 {
    match kind>>1 {
        0=>0xcc00186a,1=>0xffff0000,2=>0xf21f3a21,3=>0xf727fc05,4=>0xed663b1b,
        5=>0x9933997f,6=>0xccc8e6ff,7=>0xffffffff,9=>0x33349dfa,
        _ if kind&1!=0=>kind,
        // Native even unknown kinds use a VisualWaterInfo allocation address as ARGB.
        // Preserve the existing portable colour approximation; only this colour is unresolved,
        // not their native style0 geometry, material, layers or RealisticWater preference.
        _=>0xcc00186a,
    }
}
fn rgba(argb:u32)->[f32;4] {
    let [a,r,g,b]=argb.to_be_bytes().map(|v|f32::from(v)/255.0);
    [r.powf(2.2),g.powf(2.2),b.powf(2.2),a]
}
fn average(a:u32,b:u32)->u32 {(a>>1&0x7f7f7f7f)+(b>>1&0x7f7f7f7f)}
/// DS100401a9..100401ed: cubic slope-alpha attenuation, no upper clamp.
fn normal_alpha(alpha:u8,normal_y:f32)->u8 {
    let t=((f64::from(normal_y)-0.8999999761581421)*10.0) as f32;
    let factor=if t<0.0 {0.0}else{(f64::from(t)*f64::from(t)*f64::from(t)) as f32};
    (f64::from(alpha)*f64::from(factor)).trunc() as i64 as u8
}
fn sub(a:[f32;3],b:[f32;3])->[f32;3] {std::array::from_fn(|i|a[i]-b[i])}
fn cross(a:[f32;3],b:[f32;3])->[f32;3] {[a[1]*b[2]-a[2]*b[1],a[2]*b[0]-a[0]*b[2],a[0]*b[1]-a[1]*b[0]]}
fn length(a:[f32;3])->f32 {a.iter().map(|&v|f64::from(v)*f64::from(v)).sum::<f64>().sqrt() as f32}
fn midpoint(a:Point,b:Point)->Point {
    let mut vertex=a.vertex;
    vertex.pos=std::array::from_fn(|i|a.vertex.pos[i]+(b.vertex.pos[i]-a.vertex.pos[i])*0.5);
    vertex.normal=std::array::from_fn(|i|(a.vertex.normal[i]+b.vertex.normal[i])*0.5);
    vertex.uv=std::array::from_fn(|i|(a.vertex.uv[i]+b.vertex.uv[i])*0.5);
    Point {vertex,argb:average(a.argb,b.argb)}
}
fn children([a,b,c]:[Point;3])->[[Point;3];4] {
    let ab=midpoint(a,b);let bc=midpoint(b,c);let ca=midpoint(c,a);
    [[a,ab,ca],[b,bc,ab],[c,ca,bc],[ab,bc,ca]]
}
fn magnitude(points:[Point;3])->f32 {length(cross(sub(points[1].vertex.pos,points[0].vertex.pos),sub(points[2].vertex.pos,points[0].vertex.pos)))}
fn initial(points:[Point;3],split:bool,depth:u32,vertices:&mut Vec<Point>,triangles:&mut Vec<Triangle>)->Result<(), &'static str> {
    if depth>24 {return Err("liquid subdivision overflow");}
    if split&&magnitude(points)>10000.0 {
        for mut triangle in children(points) {
            // 1003a3b4 passes kind, not vertex colour, to 1003a0d6's colour switch.
            for point in &mut triangle {point.argb=points[0].argb;}
            initial(triangle,true,depth+1,vertices,triangles)?;
        }
    } else {
        let center:[f64;3]=std::array::from_fn(|i|(f64::from(points[0].vertex.pos[i])+f64::from(points[1].vertex.pos[i])+f64::from(points[2].vertex.pos[i]))/3.0);
        let cell=(center[0]/100.0).trunc()+(-center[2]/100.0).trunc()*60.0;
        if !(0.0..3600.0).contains(&cell) {return Ok(());}
        let mut triangle=[0;3];
        for (index,point) in triangle.iter_mut().zip(points) {
            *index=merge_point(point,vertices)?;
        }
        triangles.push(Triangle {indices:triangle,cell:cell as i32});
    }
    Ok(())
}
fn same_position(a:&Vertex,b:&Vertex)->bool {
    // Double @10089e08 is 0.20000000298023224 (the promoted native 0.2f).
    a.pos.iter().zip(b.pos).all(|(a,b)|(*a-b).abs()<0.2)
}
fn compatible(a:&Vertex,b:&Vertex)->bool {same_position(a,b)&&((a.normal[1]>0.2)==(b.normal[1]>0.2))}
fn merge_point(point:Point,vertices:&mut Vec<Point>)->Result<u32,&'static str> {
    // ponytail: native backwards O(n²) merge; spatial bins if authored liquid loading becomes costly.
    if let Some(index)=(1..vertices.len()).rev().find(|&i|same_position(&vertices[i].vertex,&point.vertex)) {
        vertices[index].vertex.normal=std::array::from_fn(|i|(vertices[index].vertex.normal[i]+point.vertex.normal[i])*0.5);
        vertices[index].argb=average(vertices[index].argb,point.argb);
        return Ok(index as u32);
    }
    if vertices.len()>=u16::MAX as usize {return Err("native liquid vertex limit");}
    let index=vertices.len() as u32;vertices.push(point);Ok(index)
}
fn refine(points:[Point;3],split:bool,vertices:&mut Vec<Vertex>,indices:&mut Vec<u32>,colors:&mut Vec<u32>) {
    let edge=|a:usize,b:usize|length(sub(points[a].vertex.pos,points[b].vertex.pos));
    if split&&(magnitude(points)>110.0||edge(0,1)>20.0||edge(1,2)>20.0||edge(2,0)>20.0) {
        let mut children=children(points);
        // DS1003e94f passes parent colour0 to all three original corners.
        children[1][0].argb=points[0].argb;children[2][0].argb=points[0].argb;
        for triangle in children {refine(triangle,true,vertices,indices,colors);}
    } else {
        for point in points {
            let index=if let Some(index)=(1..vertices.len()).rev().find(|&i|compatible(&vertices[i],&point.vertex)) {
                vertices[index].normal=std::array::from_fn(|i|(vertices[index].normal[i]+point.vertex.normal[i])*0.5);
                colors[index]=average(colors[index],point.argb);index as u32
            } else {
                let index=vertices.len() as u32;vertices.push(point.vertex);colors.push(point.argb);index
            };
            indices.push(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn triangle(size:f32)->SourceTriangle {
        SourceTriangle {kind:0,vertices:[[0.0,0.0,0.0],[size,0.0,0.0],[0.0,0.0,-size]].map(|pos|Vertex {pos,..Default::default()})}
    }
    #[test]
    fn optional_ground_grass_excludes_liquid_and_shoreline_bounds() {
        let surface = Surface::new(0, 0, vec![triangle(8.0)]).unwrap();
        assert!(surface.overlaps_ground_cell(1.0, -1.0, 1.0, -0.1));
        assert!(surface.overlaps_ground_cell(7.5, -7.5, 1.0, 0.0));
        assert!(!surface.overlaps_ground_cell(9.0, -1.0, 1.0, -0.1));
        assert!(!surface.overlaps_ground_cell(1.0, -1.0, 1.0, 0.1));
    }
    #[test]
    fn lava_fx41_sources_follow_native_near_cell_lifecycle() {
        let surface = Surface::new(0, 1, vec![SourceTriangle { kind: 2, ..triangle(30.0) }]).unwrap();
        let mut spheres = vec![];
        surface.heat_spheres([0.0; 3], true, &mut spheres);
        assert_eq!(spheres.len(), 1);
        let radius = (20.0f32 * 20.0 + 10.0 * 10.0).sqrt();
        assert_eq!(spheres[0], [10.0, -radius * 0.25, -10.0, radius]);
        surface.heat_spheres([500.0, 0.0, -500.0], true, &mut spheres);
        assert!(spheres.is_empty(), "recognition-ID rebuild removes old emitters outside the near neighbourhood");
        surface.heat_spheres([0.0; 3], false, &mut spheres);
        assert!(spheres.is_empty(), "disabled RealisticWater takes the native coarse branch without FX41 creation");
    }
    #[test]
    fn native_default_kinds_refine_as_water() {
        for kind in [338,960] {
            let surface=Surface::new(0,0,vec![SourceTriangle {kind,..triangle(80.0)}]).unwrap();
            assert_eq!(style(kind),Some(0));
            let (mut vertices,mut indices,mut colors)=(Vec::new(),Vec::new(),Vec::new());
            surface.geometry([0.0;3],false,&mut vertices,&mut indices,&mut colors);
            let coarse=indices.len();
            surface.geometry([0.0;3],true,&mut vertices,&mut indices,&mut colors);
            assert!(indices.len()>coarse,"native default kind {kind} must consume RealisticWater refinement");
        }
    }
    #[test]
    fn native_liquid_refinement_is_live_and_camera_cell_bound() {
        let surface=Surface::new(0,0,vec![triangle(80.0)]).unwrap();
        let shared=surface.clone();
        assert!(Arc::ptr_eq(&surface.vertices,&shared.vertices));
        assert!(Arc::ptr_eq(&surface.triangles,&shared.triangles));
        let (mut vertices,mut indices,mut colors)=(Vec::new(),Vec::new(),Vec::new());
        surface.geometry([0.0;3],false,&mut vertices,&mut indices,&mut colors);
        assert_eq!(indices.len(),3);
        surface.geometry([0.0;3],true,&mut vertices,&mut indices,&mut colors);
        assert!(indices.len()>3);assert!(vertices.iter().all(|v|v.normal[1]>0.99));
        surface.geometry([500.0,0.0,-500.0],true,&mut vertices,&mut indices,&mut colors);
        assert_eq!(indices.len(),3);
        assert_eq!(Surface::cell([199.9,0.0,-200.1]),[1,2]);
        assert_eq!(average(0xffffffff,0xffffffff),0xfefefefe);
        assert_eq!(normal_alpha(255,1.0),255);
        assert_eq!(normal_alpha(255,0.9),0);
        assert_eq!(normal_alpha(255,0.95),31);
        let mut bad=triangle(1.0);bad.vertices[0].pos[0]=f32::NAN;
        assert!(Surface::new(0,0,vec![bad]).is_err());
        let mut lava=triangle(80.0);lava.kind=2;
        Surface::new(1,1,vec![lava]).unwrap().geometry([0.0;3],true,&mut vertices,&mut indices,&mut colors);
        assert_eq!(indices.len(),3,"lava bypasses realistic subdivision");
        let even=Surface::new(0,0,vec![triangle(180.0)]).unwrap();
        even.geometry([0.0;3],false,&mut vertices,&mut indices,&mut colors);
        assert_eq!(indices.len(),12,"native initial 10000 cross threshold");
        let mut low=Vertex {pos:[1.0,0.0,0.0],normal:[1.0,0.1,0.0],..Default::default()};
        let high=Vertex {normal:[0.0,1.0,0.0],..low};
        assert!(same_position(&low,&high));
        assert!(!compatible(&low,&high),"fine merge preserves normal-y categories");
        low.pos[0]+=0.2;
        assert!(!same_position(&low,&high),"native merge has a strict epsilon");
    }
}
