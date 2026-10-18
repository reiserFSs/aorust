// Screen-only SSR and static observed environment; ao-vk ssr/env_accum/env_filter concepts (MIT).
struct Params {
 inverse_vp: mat4x4<f32>, vp: mat4x4<f32>, eye: vec4<f32>,
 sky: vec4<f32>, ground: vec4<f32>, control: vec4<f32>, wet: vec4<f32>, area: vec4<f32>, fog: vec4<f32>,
}
@group(0) @binding(0) var<uniform> u: Params;
@group(0) @binding(1) var scene: texture_2d<f32>;
@group(0) @binding(2) var depth: texture_depth_2d;
@group(0) @binding(3) var surface: texture_2d<f32>;
@group(0) @binding(4) var admission: texture_2d<f32>;
@group(0) @binding(5) var history: texture_2d<f32>;
@group(0) @binding(6) var roof: texture_2d<f32>;
@vertex fn vs(@builtin(vertex_index) id:u32)->@builtin(position) vec4<f32>{
 let p=vec2<f32>(f32((id<<1u)&2u),f32(id&2u)); return vec4<f32>(p*2.0-1.0,0.0,1.0);
}
fn world(pixel:vec2<i32>, z:f32)->vec3<f32>{
 let uv=(vec2<f32>(pixel)+0.5)/vec2<f32>(textureDimensions(depth));
 let q=u.inverse_vp*vec4<f32>(uv.x*2.0-1.0,1.0-uv.y*2.0,z,1.0);return q.xyz/q.w;
}
fn oct_decode(uv:vec2<f32>)->vec3<f32>{
 let f=uv*2.0-1.0; var n=vec3<f32>(f,1.0-abs(f.x)-abs(f.y));
 if n.z<0.0 {n=vec3<f32>((1.0-abs(n.yx))*select(vec2<f32>(-1.0),vec2<f32>(1.0),n.xy>=vec2<f32>(0.0)),n.z);} return normalize(n);
}
fn oct_encode(n:vec3<f32>)->vec2<f32>{
 var p=n.xy/(abs(n.x)+abs(n.y)+abs(n.z));
 if n.z<0.0 {p=(1.0-abs(p.yx))*select(vec2<f32>(-1.0),vec2<f32>(1.0),p>=vec2<f32>(0.0));} return p*0.5+0.5;
}
@fragment fn accumulate(@builtin(position) pos:vec4<f32>)->@location(0) vec4<f32>{
 let pixel=vec2<i32>(pos.xy); var old=textureLoad(history,pixel,0)*vec4<f32>(1.0,1.0,1.0,u.control.z);
 if u.control.w>0.5 {old=vec4<f32>(0.0);}
 let ray=oct_decode(pos.xy/128.0); let clip=u.vp*vec4<f32>(ray,0.0);
 if clip.w<=0.0001 {return old;}
 let ndc=clip.xy/clip.w; if max(abs(ndc.x),abs(ndc.y))>=0.98 {return old;}
 let uv=vec2<f32>(ndc.x*0.5+0.5,0.5-ndc.y*0.5);let p=vec2<i32>(uv*vec2<f32>(textureDimensions(depth)));
 // No distance heuristic: only primary-observed STATIC opaque/cutout coverage is admitted.
 let m=textureLoad(admission,p,0); if m.b<0.999 || m.a<0.999 || textureLoad(depth,p,0)>=1.0 {return old;}
 // Unfiltered load prevents a neighbouring actor/effect leaking into a static texel.
 let c=clamp(textureLoad(scene,p,0).rgb,vec3<f32>(0.0),vec3<f32>(1.0));
 let w=0.12;let confidence=mix(old.a,1.0,w);
 return vec4<f32>((old.rgb*old.a*(1.0-w)+c*w)/max(confidence,0.0001),confidence);
}
fn environment(ray:vec3<f32>,rough:f32)->vec3<f32>{
 let analytic=mix(u.ground.rgb,u.sky.rgb,smoothstep(-0.25,0.5,ray.y));
 if u.control.y<0.5 {return analytic;}
 // ponytail: 16 bounded GGX taps at the receiver avoid a second prefilter atlas.
 let up=select(vec3<f32>(0.0,1.0,0.0),vec3<f32>(1.0,0.0,0.0),abs(ray.y)>0.99);
 let tx=normalize(cross(up,ray));let ty=cross(ray,tx);var sum=vec3<f32>(0.0);var sure=0.0;var total=0.0;
 let a2=pow(max(rough,0.08),4.0);
 for(var i=0u;i<16u;i+=1u){
 let x=(f32(i)+0.5)/16.0;let phi=6.2831853*fract(f32(i)*0.618034);
 let ct=sqrt((1.0-x)/(1.0+(a2-1.0)*x));let st=sqrt(max(1.0-ct*ct,0.0));
 let h=tx*(st*cos(phi))+ty*(st*sin(phi))+ray*ct;let l=reflect(-ray,h);let weight=max(dot(ray,l),0.0);
 let uv=oct_encode(l);let p=clamp(vec2<i32>(uv*128.0),vec2<i32>(0),vec2<i32>(127));let c=textureLoad(history,p,0);
 sum+=c.rgb*c.a*weight;sure+=c.a*weight;total+=weight;
 }
 return mix(analytic,sum/max(sure,0.0001),clamp(sure/max(total,0.0001),0.0,1.0));
}
@fragment fn reflection(@builtin(position) pos:vec4<f32>)->@location(0) vec4<f32>{
 let pixel=vec2<i32>(pos.xy);let d=textureLoad(depth,pixel,0);let m=textureLoad(admission,pixel,0);let s=textureLoad(surface,pixel,0);
 if d>=1.0 || m.a<0.999 || length(s.xyz)<0.5 {return vec4<f32>(0.0);}
 let p=world(pixel,d);let v=normalize(p-u.eye.xyz);var n=normalize(s.xyz);if dot(n,v)>0.0 {n=-n;}
 var strength=m.r;var rough=clamp(s.w,0.04,1.0);
 if u.wet.x>0.0 && m.b>0.999 && n.y>0.7 {
 let uv=(p.xz-u.area.xy)*u.area.z;
 if all(uv>=vec2<f32>(0.0)) && all(uv<vec2<f32>(1.0)) {
 let r=textureLoad(roof,vec2<i32>(uv*vec2<f32>(textureDimensions(roof))),0).r;
 if p.y+0.005>=r {let wet=u.wet.x*smoothstep(0.7,0.97,n.y);strength=max(strength,wet*0.3);rough=mix(rough,0.12,wet);}
 }}
 if strength<0.004 {return vec4<f32>(0.0);}
 let ray=reflect(v,n);var colour=environment(ray,rough);
 if u.control.x>0.5 && rough<0.75 {
 let origin=p+n*0.05;var previous=0.0;
 for(var i=1u;i<=48u;i+=1u){
 let t=60.0*pow(f32(i)/48.0,2.0);let q=origin+ray*t;let clip=u.vp*vec4<f32>(q,1.0);
 if clip.w<=0.0 {break;}let ndc=clip.xy/clip.w;let uv=vec2<f32>(ndc.x*0.5+0.5,0.5-ndc.y*0.5);
 if any(uv<=vec2<f32>(0.0)) || any(uv>=vec2<f32>(1.0)){break;}
 let hp=vec2<i32>(uv*vec2<f32>(textureDimensions(depth)));let hd=textureLoad(depth,hp,0);
 if hd<1.0 {
 let hit_position=world(hp,hd);let behind=length(q-u.eye.xyz)-length(hit_position-u.eye.xyz);
 if behind>0.0 && behind<max(0.15,(t-previous)*1.5) {
 // A ray hit is usable only if this exact current primary sample is admitted.
 let source=textureLoad(admission,hp,0);
 if source.a>=0.999 {let edge=smoothstep(0.0,0.08,min(min(uv.x,uv.y),min(1.0-uv.x,1.0-uv.y)));colour=mix(colour,textureLoad(scene,hp,0).rgb,edge*(1.0-smoothstep(0.45,0.75,rough)));}break;
 }}previous=t;
 }}
 // Source already contains native fog; apply receiver extinction as well, never undo either.
 let fresnel=0.25+0.75*pow(1.0-clamp(dot(-v,n),0.0,1.0),3.0);
 return vec4<f32>(mix(colour,u.fog.rgb,clamp(m.g,0.0,1.0)),clamp(strength*fresnel*(1.0-m.g),0.0,0.8));
}
