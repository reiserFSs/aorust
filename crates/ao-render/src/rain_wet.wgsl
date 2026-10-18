// Procedural world anchoring inspired by ao-vk rain.vert / splash.vert (MIT).
struct Params {
 inverse_vp: mat4x4<f32>, vp: mat4x4<f32>, eye_time: vec4<f32>,
 wind_rain: vec4<f32>, area: vec4<f32>, wet: vec4<f32>, sun: vec4<f32>, fog: vec4<f32>, colour: vec4<f32>,
 drift: vec4<f32>, surface_colour: vec4<f32>,
}
@group(0) @binding(0) var<uniform> u: Params;
@group(0) @binding(1) var depth: texture_depth_2d;
@group(0) @binding(2) var mask: texture_2d<f32>;
@group(0) @binding(3) var roof: texture_2d<f32>;
@group(0) @binding(4) var water_positions: texture_2d<f32>;
@group(0) @binding(5) var water_normals: texture_2d<f32>;
@vertex fn vs(@builtin(vertex_index) id: u32) -> @builtin(position) vec4<f32> {
 let xy = vec2<f32>(f32((id << 1u) & 2u), f32(id & 2u));
 return vec4<f32>(xy * 2.0 - 1.0, 0.0, 1.0);
}
fn world(pixel: vec2<i32>, d: f32) -> vec3<f32> {
 let uv = (vec2<f32>(pixel) + 0.5) / vec2<f32>(textureDimensions(depth));
 let q = u.inverse_vp * vec4<f32>(uv.x*2.0-1.0, 1.0-uv.y*2.0, d, 1.0);
 return q.xyz / q.w;
}
fn exposed(p: vec3<f32>) -> f32 {
 let uv = (p.xz - u.area.xy) * u.area.z;
 if any(uv < vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0)) { return 0.0; }
 let r = textureLoad(roof, vec2<i32>(uv * vec2<f32>(textureDimensions(roof))), 0).r;
 return select(0.0, 1.0, p.y + 0.005 >= r);
}
fn hash(p: vec3<f32>) -> f32 { return fract(sin(dot(p, vec3<f32>(127.1,311.7,74.7))) * 43758.5453); }
fn normal(pixel: vec2<i32>, p: vec3<f32>) -> vec3<f32> {
 let size = vec2<i32>(textureDimensions(depth));
 let px = clamp(pixel+vec2<i32>(1,0),vec2<i32>(0),size-vec2<i32>(1));
 let py = clamp(pixel+vec2<i32>(0,1),vec2<i32>(0),size-vec2<i32>(1));
 let dx = world(px,textureLoad(depth,px,0))-p;
 let dy = world(py,textureLoad(depth,py,0))-p;
 let cross_normal = cross(dx,dy);
 let n = cross_normal / max(length(cross_normal),0.000001);
 return n * select(-1.0,1.0,n.y >= 0.0);
}
fn receiver(pixel: vec2<i32>) -> vec4<f32> {
 let d = textureLoad(depth,pixel,0);
 let m = textureLoad(mask,pixel,0);
 if d >= 1.0 || m.a < 0.5 { return vec4<f32>(0.0); }
 let p = world(pixel,d);
 let n = normal(pixel,p);
 let flat = smoothstep(0.7,0.97,n.y);
 // The native directional attachment retains native fog fraction in B.
 return vec4<f32>(p,flat*exposed(p)*(1.0-m.b));
}
@fragment fn wet(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
 let r = receiver(vec2<i32>(pos.xy));
 return vec4<f32>(vec3<f32>(1.0 - 0.24*u.wet.x*r.w),1.0);
}
@fragment fn sheen(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
 let pixel = vec2<i32>(pos.xy); let r = receiver(pixel);
 if r.w <= 0.0 { return vec4<f32>(0.0); }
 let n = normal(pixel,r.xyz);
 let v = normalize(u.eye_time.xyz-r.xyz);
 let l = u.sun.xyz/max(length(u.sun.xyz),0.0001);
 let h = (v+l)/max(length(v+l),0.0001);
 let patches = smoothstep(0.35,0.7,hash(floor(r.xyz*0.38)));
 let gloss = pow(max(dot(n,h),0.0),128.0)*max(l.y,0.0);
 let fresnel = pow(1.0-max(dot(n,v),0.0),5.0);
 let light = (gloss*0.22 + fresnel*patches*0.045)*r.w*u.wet.x;
 return vec4<f32>(light*max(u.colour.rgb,vec3<f32>(0.05)),0.0);
}
@fragment fn water_rings(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
 if any(textureDimensions(water_positions) != textureDimensions(depth)) { return vec4<f32>(0.0); }
 let pixel=vec2<i32>(pos.xy);
 let water=textureLoad(water_positions,pixel,0);
 let n=textureLoad(water_normals,pixel,0);
 if water.w<=0.0 || n.y<=0.7 { return vec4<f32>(0.0); }
 let slope=rain_ring_normal(water.xyz,u.eye_time.w,u.wind_rain.w);
 let distance=length(water.xyz-u.eye_time.xyz);
 // Native water does not own the directional attachment: its B channel may
 // retain fog from the opaque background. Use this liquid's own transmission,
 // matching water_optics.wgsl, rather than extinguishing it with background fog.
 let fog=clamp((u.fog.y-distance)/max(u.fog.y-u.fog.x,0.001),0.0,1.0);
 let alpha=min(length(slope)*5.0,0.5)*exposed(water.xyz)*fog;
 return vec4<f32>(u.surface_colour.rgb,alpha);
}

struct Drop {
 @builtin(position) position: vec4<f32>,
 @location(0) corner: vec2<f32>,
 @location(1) alpha: f32,
}
@vertex fn rain_vertex(@builtin(vertex_index) vertex: u32, @builtin(instance_index) index: u32) -> Drop {
 let seed = vec3<f32>(hash(vec3<f32>(f32(index),1.0,0.0)),hash(vec3<f32>(f32(index),2.0,0.0)),hash(vec3<f32>(f32(index),3.0,0.0)));
 let lo = u.eye_time.xyz - vec3<f32>(30.0);
 let motion = seed*60.0+u.drift.xyz-lo;
 let head = lo + motion-floor(motion/60.0)*60.0;
 let tail = head-vec3<f32>(u.wind_rain.x,-18.0,u.wind_rain.z)*0.028;
 let a = u.vp*vec4<f32>(tail,1.0);
 let b = u.vp*vec4<f32>(head,1.0);
 var out: Drop;
 out.position=vec4<f32>(2.0,2.0,2.0,1.0); out.alpha=0.0; out.corner=vec2<f32>(0.0);
 if a.w <= 0.05 || b.w <= 0.05 { return out; }
 let corners=array<vec2<f32>,6>(vec2<f32>(0.0,-1.0),vec2<f32>(0.0,1.0),vec2<f32>(1.0,1.0),vec2<f32>(0.0,-1.0),vec2<f32>(1.0,1.0),vec2<f32>(1.0,-1.0));
 let c=corners[vertex];
 let dimensions=vec2<f32>(textureDimensions(depth));
 let delta=(b.xy/b.w-a.xy/a.w)*dimensions;
 let along=delta/max(length(delta),0.0001);
 var clip=mix(a,b,c.x);
 clip.x+=-along.y*c.y*1.25/dimensions.x*clip.w;
 clip.y+=along.x*c.y*1.25/dimensions.y*clip.w;
 let distance=length(head-u.eye_time.xyz);
 let fade=1.0-smoothstep(20.0,30.0,max(abs(head.x-u.eye_time.x),max(abs(head.y-u.eye_time.y),abs(head.z-u.eye_time.z))));
 let fog=1.0-smoothstep(u.fog.x,max(u.fog.y,u.fog.x+0.01),distance);
 out.position=clip; out.corner=c;
 out.alpha=0.3*fade*fog*exposed(head)*smoothstep(0.3,1.0,distance);
 return out;
}
@fragment fn rain_streak(drop: Drop) -> @location(0) vec4<f32> {
 let pixel=vec2<i32>(drop.position.xy);
 if drop.position.z > textureLoad(depth,pixel,0)+0.00001 { discard; }
 let alpha=drop.alpha*pow(max(1.0-abs(drop.corner.y),0.0),1.5)*sin(drop.corner.x*3.14159265);
 return vec4<f32>(max(u.colour.rgb,vec3<f32>(0.1))*0.7,alpha);
}

// Current admitted depth, not a terrain-height guess, locates each impact.
fn probe_surface(p: vec3<f32>) -> vec2<f32> {
 let c=u.vp*vec4<f32>(p,1.0);
 if c.w<=0.05 { return vec2<f32>(0.0); }
 let ndc=c.xyz/c.w;
 if any(abs(ndc.xy)>=vec2<f32>(0.999)) || ndc.z<=0.0 || ndc.z>=1.0 { return vec2<f32>(0.0); }
 let uv=ndc.xy*vec2<f32>(0.5,-0.5)+vec2<f32>(0.5);
 let pixel=vec2<i32>(uv*vec2<f32>(textureDimensions(depth)));
 let d=textureLoad(depth,pixel,0);
 if d>=1.0 { return vec2<f32>(0.0); }
 return vec2<f32>(ndc.z-d,1.0);
}
struct Splash {
 @builtin(position) position: vec4<f32>,
 @location(0) corner: vec2<f32>,
 @location(1) age: f32,
 @location(2) alpha: f32,
}
@vertex fn splash_vertex(@builtin(vertex_index) vertex: u32,@builtin(instance_index) index: u32) -> Splash {
 var out: Splash;
 out.position=vec4<f32>(2.0,2.0,2.0,1.0); out.corner=vec2<f32>(0.0); out.age=0.0; out.alpha=0.0;
 let seed=hash(vec3<f32>(f32(index),19.0,7.0));
 let phase=u.eye_time.w*4.0+seed;
 let age=fract(phase);
 let random=vec2<f32>(hash(vec3<f32>(f32(index),floor(phase),11.0)),hash(vec3<f32>(f32(index),floor(phase),23.0)))*4096.0;
 let lo=u.eye_time.xz-vec2<f32>(22.0);
 let xz=lo+(random-lo)-floor((random-lo)/44.0)*44.0;
 var high=u.eye_time.y+16.0;
 var low=high;
 var front=false;
 var found=false;
 for(var step=0u;step<28u;step++) {
  let y=u.eye_time.y+16.0-f32(step)*2.0;
  let probe=probe_surface(vec3<f32>(xz.x,y,xz.y));
  if probe.y==0.0 { continue; }
  if probe.x>0.0 {
   if front { low=y; found=true; }
   break;
  }
  high=y; front=true;
 }
 if !found { return out; }
 for(var refine=0u;refine<8u;refine++) {
  let y=(high+low)*0.5;
  let probe=probe_surface(vec3<f32>(xz.x,y,xz.y));
  if probe.y>0.0 && probe.x>0.0 { low=y; } else { high=y; }
 }
 let p=vec3<f32>(xz.x,high,xz.y);
 let clip=u.vp*vec4<f32>(p,1.0);
 let uv=clip.xy/clip.w*vec2<f32>(0.5,-0.5)+vec2<f32>(0.5);
 if clip.w<=0.05 || any(uv<=vec2<f32>(0.0)) || any(uv>=vec2<f32>(1.0)) { return out; }
 let pixel=vec2<i32>(uv*vec2<f32>(textureDimensions(depth)));
 if textureLoad(mask,pixel,0).a<0.5 { return out; }
 let actual=world(pixel,textureLoad(depth,pixel,0));
 if length(actual-p)>0.15+0.003*clip.w || normal(pixel,actual).y<0.7 || exposed(p)<0.5 { return out; }
 let look=u.eye_time.xyz-p;
 let side=vec3<f32>(look.z,0.0,-look.x)/max(length(look.xz),0.001);
 let corners=array<vec2<f32>,6>(vec2<f32>(-1.0,-0.05),vec2<f32>(1.0,-0.05),vec2<f32>(1.0,1.0),vec2<f32>(-1.0,-0.05),vec2<f32>(1.0,1.0),vec2<f32>(-1.0,1.0));
 let c=corners[vertex];
 out.position=u.vp*vec4<f32>(p+side*c.x*0.25+vec3<f32>(0.0,c.y*0.6,0.0),1.0);
 out.corner=c; out.age=age;
 out.alpha=0.35*(1.0-smoothstep(u.fog.x,max(u.fog.y,u.fog.x+0.01),length(look)))*(1.0-textureLoad(mask,pixel,0).b);
 return out;
}
@fragment fn splash_fragment(splash: Splash) -> @location(0) vec4<f32> {
 // Evaluate footprint before the depth discard; narrow crowns and droplets
 // otherwise vanish between pixel centres in the native-sized world view.
 let footprint=fwidth(splash.corner);
 if splash.position.z>textureLoad(depth,vec2<i32>(splash.position.xy),0)+0.00001 { discard; }
 let a=splash.age;
 let spread=0.15+0.75*sqrt(a);
 let height=0.4*sin(3.14159265*min(a*1.6,1.0));
 let width=max(0.09,footprint.x*0.65);
 let crown=exp(-pow((abs(splash.corner.x)-spread)/width,2.0))*step(0.0,splash.corner.y)*(1.0-smoothstep(height*0.6,max(height+footprint.y*0.5,0.001),splash.corner.y));
 var droplets=0.0;
 for(var k=0u;k<4u;k++) {
  let side=(f32(k&1u)*2.0-1.0)*(0.35+0.12*f32(k));
  let point=vec2<f32>(side*a*1.2,(2.2+0.4*f32(k))*a*(1.0-a));
  let radius=length((splash.corner-point)*vec2<f32>(1.0,0.8));
  let edge=max(0.03,length(footprint)*0.5);
  droplets=max(droplets,1.0-smoothstep(max(0.08-edge,0.0),0.08+edge,radius));
 }
 let alpha=splash.alpha*max(crown*0.8,droplets)*(1.0-a);
 return vec4<f32>(u.surface_colour.rgb,alpha);
}
