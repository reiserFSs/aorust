// Adapted from ao-vk taa.frag, Copyright (c) 2026 wannabuh, MIT (THIRD_PARTY).
struct Params { size: vec4<f32>, control: vec4<f32> }
@group(0) @binding(0) var current: texture_2d<f32>;
@group(0) @binding(1) var history: texture_2d<f32>;
@group(0) @binding(2) var motion: texture_2d<f32>;
@group(0) @binding(3) var identity: texture_2d<u32>;
@group(0) @binding(4) var old_identity: texture_2d<u32>;
@group(0) @binding(5) var depth: texture_depth_2d;
@group(0) @binding(6) var old_depth: texture_2d<f32>;
@group(0) @binding(7) var<uniform> params: Params;
@group(0) @binding(8) var native_fog: texture_2d<f32>;
@group(0) @binding(9) var<storage,read> actor_visibility_changed: u32;
@vertex fn vs(@builtin(vertex_index) i:u32)->@builtin(position) vec4<f32> {
    return vec4<f32>(vec2<f32>(f32((i<<1u)&2u),f32(i&2u))*2.0-1.0,0.0,1.0);
}
fn bounded(p:vec2<i32>)->vec2<i32>{return clamp(p,vec2<i32>(0),vec2<i32>(params.size.xy)-1);}
fn yc(c:vec3<f32>)->vec3<f32>{return vec3<f32>(dot(c,vec3<f32>(0.25,0.5,0.25)),0.5*(c.r-c.b),dot(c,vec3<f32>(-0.25,0.5,-0.25)));}
fn rgb(c:vec3<f32>)->vec3<f32>{return vec3<f32>(c.x+c.y-c.z,c.x+c.z,c.x-c.y-c.z);}
fn luma(c:vec3<f32>)->f32{return dot(c,vec3<f32>(0.2126,0.7152,0.0722));}
fn cat_weights(f:f32)->vec4<f32>{return vec4<f32>(f*(-0.5+f*(1.0-0.5*f)),1.0+f*f*(-2.5+1.5*f),f*(0.5+f*(2.0-1.5*f)),f*f*(-0.5+0.5*f));}
// History and current colour use the same native gamma-space fog colour;
// changing that colour resets history before this resolve is encoded.
fn surface(colour:vec3<f32>,fog:f32)->vec3<f32>{
    return clamp((colour-fog*params.control.yzw)/max(1.0-fog,0.00001),vec3<f32>(0.0),vec3<f32>(1.0));
}
// Every reconstruction tap must match the currently admitted surface. Checking only the
// nearest history pixel lets a disappeared actor survive in a Catmull-Rom side lobe.
fn reconstruct(pos:vec2<f32>,id:u32,z:f32,fallback:vec3<f32>)->vec3<f32>{
    let p=floor(pos-0.5); let f=pos-0.5-p; let wx=cat_weights(f.x); let wy=cat_weights(f.y);
    // Catmull-Rom always reconstructs the underlying surface, including in fog.
    // Its negative lobes cannot subtract native extinction: refogging below
    // applies this frame's unchanged fraction after all reconstruction/clipping.
    var sum=vec3<f32>(0.0);
    for(var y=0;y<4;y++){for(var x=0;x<4;x++){
        let q=bounded(vec2<i32>(p)+vec2<i32>(x-1,y-1));
        let d=textureLoad(old_depth,q,0);
        let valid=textureLoad(old_identity,q,0).r==id && abs(d.r-z)<max(0.0002,0.002*abs(z)) && d.g<1.0;
        let recovered=surface(textureLoad(history,q,0).rgb,d.g);
        sum+=select(fallback,recovered,valid)*wx[x]*wy[y];
    }} return clamp(sum,vec3<f32>(0.0),vec3<f32>(1.0));
}
struct Result { @location(0) colour:vec4<f32>, @location(1) id:u32, @location(2) depth:vec2<f32> }
@fragment fn resolve(@builtin(position) p:vec4<f32>)->Result {
    let q=vec2<i32>(p.xy); let cur=textureLoad(current,q,0); let id=textureLoad(identity,q,0).r;
    let m=textureLoad(motion,q,0); let prev=p.xy-m.xy*params.size.xy;
    let fog=textureLoad(native_fog,q,0).b;
    var c=cur.rgb;
    if actor_visibility_changed==0u && fog<1.0 && params.control.x>0.5 && m.w<1.0 && all(prev>=vec2<f32>(0.5)) && all(prev<params.size.xy-0.5) {
        let hq=bounded(vec2<i32>(prev));
        if textureLoad(old_identity,hq,0).r==id && abs(textureLoad(old_depth,hq,0).r-m.z)<max(0.0002,0.002*abs(m.z)) {
            let current_surface=surface(cur.rgb,fog);
            var mean=vec3<f32>(0.0); var square=vec3<f32>(0.0);
            for(var y=-1;y<=1;y++){for(var x=-1;x<=1;x++){
                let n=bounded(q+vec2<i32>(x,y));
                let f=textureLoad(native_fog,n,0).b;
                let valid=textureLoad(identity,n,0).r==id && f<1.0;
                let sample=select(current_surface,surface(textureLoad(current,n,0).rgb,f),valid);
                let v=yc(sample); mean+=v; square+=v*v;
            }}
            mean/=9.0; let sigma=sqrt(max(square/9.0-mean*mean,vec3<f32>(0.0)));
            let extent=max(1.25*sigma,vec3<f32>(0.0001));
            let offset=yc(reconstruct(prev,id,m.z,current_surface))-mean;
            let units=abs(offset/extent); let limit=max(1.0,max(units.x,max(units.y,units.z)));
            let h=rgb(mean+offset/limit);
            let weight=mix(clamp(0.10+length(m.xy*params.size.xy)*0.002,0.1,0.4),1.0,m.w);
            let wc=weight/(1.0+luma(current_surface)*(1.0-m.w));
            let wh=(1.0-weight)/(1.0+max(luma(h),0.0)*(1.0-m.w));
            let resolved_surface=clamp((current_surface*wc+h*wh)/max(wc+wh,0.00001),vec3<f32>(0.0),vec3<f32>(1.0));
            // Keep current native UNORM quantization residual while changing only
            // the surface term. The exact current fog term/extinction is retained.
            c=cur.rgb+(resolved_surface-current_surface)*(1.0-fog);
        }
    }
    return Result(vec4<f32>(clamp(c,vec3<f32>(0.0),vec3<f32>(1.0)),cur.a),id,vec2<f32>(textureLoad(depth,q,0),fog));
}
