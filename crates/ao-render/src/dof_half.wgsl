// Golden-spiral aperture adapted from ao-vk (MIT, Copyright 2026 wannabuh).
// See THIRD_PARTY. All inputs are resolved, current-visible-frame textures.
struct Params { projection: vec4<f32>, effect: vec4<f32>, fog_color: vec4<f32> }
@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var depth: texture_depth_2d;
@group(0) @binding(2) var<uniform> params: Params;
@group(0) @binding(3) var fog_mask: texture_2d<f32>;
@group(0) @binding(4) var half_colour: texture_2d<f32>;
@group(0) @binding(5) var half_data: texture_2d<f32>;
@group(0) @binding(6) var tile_data: texture_2d<f32>;
@group(0) @binding(7) var near_result: texture_2d<f32>;
@group(0) @binding(8) var far_result: texture_2d<f32>;
// Device-cached immutable offsets avoid a dynamically indexed local array.
@group(0) @binding(9) var<uniform> aperture: array<vec4<f32>,48>;
@group(0) @binding(10) var nearest_sampler: sampler;
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    return vec4<f32>(array<vec2<f32>,3>(vec2(-1.0,-1.0),vec2(3.0,-1.0),vec2(-1.0,3.0))[i],0.0,1.0);
}
fn full_coord(p: vec2<i32>) -> vec2<i32> { return clamp(p,vec2(0),vec2<i32>(textureDimensions(image))-1); }
fn half_coord(p: vec2<i32>) -> vec2<i32> { return clamp(p,vec2(0),vec2<i32>(textureDimensions(half_data))-1); }
fn coc(inverse_z: f32) -> f32 {
    let t = 1.0-params.projection.z*min(inverse_z,1000.0);
    let magnitude = max(abs(t)-0.025,0.0)/0.975*params.projection.w;
    return 0.5*sign(t)*magnitude/(1.0+magnitude)*params.effect.x;
}
fn full_data(p: vec2<i32>) -> vec3<f32> {
    let q = full_coord(p);
    let d = textureLoad(depth,q,0);
    let iz = params.effect.w-d*(params.effect.w-params.fog_color.w);
    return vec3(coc(iz),clamp(textureLoad(fog_mask,q,0).b,0.0,1.0),iz);
}
struct Prepared { @location(0) colour: vec4<f32>, @location(1) data: vec4<f32> }
fn prefer(candidate: vec3<f32>, current: vec3<f32>) -> bool {
    return (candidate.x<=0.25 && candidate.z>current.z)
        || (candidate.x>0.25 && current.x>0.25 && candidate.x>current.x);
}
fn reduced_colour(p: vec2<i32>, data: vec3<f32>, selected: vec3<f32>) -> vec4<f32> {
    let layer=select(select(0,1,data.x>0.25),-1,data.x< -0.25);
    let selected_layer=select(select(0,1,selected.x>0.25),-1,selected.x< -0.25);
    if layer==selected_layer && abs(data.x-selected.x)<0.5 && data.y>=selected.y {
        return vec4(textureLoad(image,full_coord(p),0).rgb,1.0);
    }
    return vec4(0.0);
}
@fragment fn fs_prepare(@builtin(position) position: vec4<f32>) -> Prepared {
    let p = vec2<i32>(position.xy)*2;
    // Preserve focused/near silhouettes; within the far layer retain the widest
    // source aperture, so a one-pixel distant lamp survives downsampling.
    let a=full_data(p); let b=full_data(p+vec2(1,0));
    let c=full_data(p+vec2(0,1)); let d=full_data(p+vec2(1,1));
    var data=a;
    if prefer(b,data) { data=b; }
    if prefer(c,data) { data=c; }
    if prefer(d,data) { data=d; }
    let sum=reduced_colour(p,a,data)+reduced_colour(p+vec2(1,0),b,data)
        +reduced_colour(p+vec2(0,1),c,data)+reduced_colour(p+vec2(1,1),d,data);
    return Prepared(vec4(sum.rgb/max(sum.a,1.0),data.x),vec4(data.y,0.0,0.0,0.0));
}
@fragment fn fs_tiles(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let base=vec2<i32>(position.xy)*8; var radii=vec2(0.0);
    for (var y=0; y<8; y+=1) { for (var x=0; x<8; x+=1) {
        let c=textureLoad(half_colour,half_coord(base+vec2(x,y)),0).a;
        radii=max(radii,vec2(-c,c));
    }}
    return vec4(radii,0.0,0.0);
}
@fragment fn fs_dilate(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let p=vec2<i32>(position.xy); let size=vec2<i32>(textureDimensions(tile_data));
    var radii=vec2(0.0);
    // Dilate once per tile, not once for each of its 64 gather pixels.
    // One tile covers the maximum eight-half-pixel near/far aperture.
    for (var y=-1; y<=1; y+=1) { for (var x=-1; x<=1; x+=1) {
        radii=max(radii,textureLoad(tile_data,clamp(p+vec2(x,y),vec2(0),size-1),0).xy);
    }}
    return vec4(radii,0.0,0.0);
}
struct Gathered { @location(0) near: vec4<f32>, @location(1) far: vec4<f32> }
@fragment fn fs_gather(@builtin(position) position: vec4<f32>) -> Gathered {
    let p=vec2<i32>(position.xy);
    let centre_colour=textureLoad(half_colour,p,0);
    let centre=vec2(centre_colour.a,textureLoad(half_data,p,0).r);
    let original=centre_colour.rgb;
    let radius=textureLoad(tile_data,p/8,0).xy;
    if max(radius.x,radius.y)<0.25 { return Gathered(vec4(0.0),vec4(original,0.0)); }
    var near_sum=vec3(0.0); var far_sum=vec3(0.0);
    var near_weight=0.0; var far_weight=0.0; var near_cover=0.0;
    let gather_radius=max(radius.x,radius.y);
    // Uniform-area, filled aperture: exactly 48 taps, independent of MSAA.
    for (var i=0; i<48; i+=1) {
        let offset=aperture[i].xy*gather_radius;
        let q=half_coord(vec2<i32>(round(vec2<f32>(p)+offset)));
        let source=textureLoad(half_colour,q,0);
        let sample=vec2(source.a,textureLoad(half_data,q,0).r);
        // Never import a less-extinguished surface into a fogged destination.
        if sample.y<centre.y { continue; }
        let c=source.rgb;
        let brightness=dot(c,vec3(0.299,0.587,0.114));
        let highlight=1.0+4.0*brightness*brightness;
        let distance=aperture[i].z*gather_radius;
        if sample.x< -0.25 {
            let near=clamp(-sample.x-distance+0.5,0.0,1.0);
            near_sum+=c*(near*highlight); near_weight+=near*highlight;
            // Only near sources need the source-disc area normalization.
            near_cover+=near*gather_radius*gather_radius/max(sample.x*sample.x,0.25);
        } else if sample.x>0.25 && centre.x>0.25 {
            let far=clamp(sample.x-distance+0.5,0.0,1.0)*highlight;
            far_sum+=c*far; far_weight+=far;
        }
    }
    let coverage=clamp(near_cover/48.0,0.0,1.0);
    return Gathered(vec4(near_sum/max(near_weight,0.0001),coverage),
        vec4(select(original,far_sum/max(far_weight,0.0001),far_weight>0.0),radius.x));
}
@fragment fn fs_upsample(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let p=vec2<i32>(position.xy); let original=textureLoad(image,p,0); let centre=full_data(p);
    if abs(centre.x)<=0.25 { return original; }
    let hp=position.xy*0.5-0.5; let base=vec2<i32>(floor(hp)); let blend=fract(hp);
    let uv=position.xy*0.5/vec2<f32>(textureDimensions(half_colour));
    let cocs=textureGather(3,half_colour,nearest_sampler,uv);
    let fogs=textureGather(0,half_data,nearest_sampler,uv);
    var far_sum=vec3(0.0); var near_sum=vec4(0.0); var weight=0.0; var far_weight=0.0;
    for (var y=0; y<2; y+=1) { for (var x=0; x<2; x+=1) {
        let q=half_coord(base+vec2(x,y));
        // Gather order is bottom-left, bottom-right, top-right, top-left.
        let index=select(3-x,x,y==1);
        let d=vec2(cocs[index],fogs[index]);
        let spatial=select(1.0-blend.x,blend.x,x==1)*select(1.0-blend.y,blend.y,y==1);
        // Bilateral CoC/fog rejection preserves the exact focused own character.
        let bilateral=1.0/(1.0+16.0*abs(d.x-centre.x));
        let w=spatial*bilateral*select(0.0,1.0,d.y>=centre.y);
        // A focused/near half pixel must not paint actor colour into far gaps.
        let far=textureLoad(far_result,q,0);
        if d.x>0.25 { far_sum+=far.rgb*w; far_weight+=w; }
        if far.a>0.25 {
            let near=textureLoad(near_result,q,0);
            near_sum+=vec4(near.rgb*near.a,near.a)*w;
        }
        weight+=w;
    }}
    if weight<=0.0001 { return original; }
    let far=select(original.rgb,far_sum/max(far_weight,0.0001),far_weight>0.0001);
    let near=vec4(near_sum.rgb/max(near_sum.a,0.0001),near_sum.a/weight);
    let far_blend=clamp(centre.x*2.0-0.5,0.0,1.0);
    // In-focus pixels are never blurred by their own downsample footprint.
    let near_blend=near.a*clamp(abs(centre.x)*2.0-0.5,0.0,1.0);
    let result=mix(mix(original.rgb,far,far_blend),near.rgb,near_blend);
    return vec4(clamp(result,vec3(0.0),vec3(1.0)),original.a);
}
