// ao-vk particles.comp curl-potential flow, MIT (c) 2026 wannabuh.
fn gradient(c: vec3<i32>) -> vec3<f32> {
 let h=hash(bitcast<u32>(c.x)+hash(bitcast<u32>(c.y)+hash(bitcast<u32>(c.z))));
 return vec3<f32>(f32(h&1023u),f32((h>>10u)&1023u),f32((h>>20u)&1023u))/511.5-1.0;
}
fn noise(x: vec3<f32>) -> f32 {
 let i=vec3<i32>(floor(x)); let f=fract(x); let u=f*f*f*(f*(f*6.0-15.0)+10.0);
 var value=0.0;
 for(var k=0u;k<8u;k++) { let c=vec3<i32>(i32(k&1u),i32((k>>1u)&1u),i32((k>>2u)&1u));
  let w=select(1.0-u,u,vec3<bool>(c.x!=0,c.y!=0,c.z!=0)); value+=dot(gradient(i+c),f-vec3<f32>(c))*w.x*w.y*w.z; }
 return value;
}
fn potential(x: vec3<f32>) -> vec3<f32> { return vec3(noise(x),noise(x+vec3(31.416,-47.853,12.793)),noise(x+vec3(-233.145,-113.408,-185.31))); }
fn curl(x: vec3<f32>) -> vec3<f32> {
 let e=0.01;
 let dx=(potential(x+vec3(e,0,0))-potential(x-vec3(e,0,0)))/(2.0*e);
 let dy=(potential(x+vec3(0,e,0))-potential(x-vec3(0,e,0)))/(2.0*e);
 let dz=(potential(x+vec3(0,0,e))-potential(x-vec3(0,0,e)))/(2.0*e);
 return vec3(dy.z-dz.y,dz.x-dx.z,dx.y-dy.x);
}
@compute @workgroup_size(64)
fn simulate(@builtin(global_invocation_id) gid: vec3<u32>) {
 let i=gid.x; if(i>=128u){return;} var c=input[i]; let dt=p.clock.x;
 if(!owner_admitted(p.admission.x)){var empty:Cell;output[i]=empty;return;}
 if(c.thermal_pressure.w<=0.0 || c.position_age.w>=c.thermal_pressure.w) {
  // Deterministic per-slot staggering; frozen frames never spontaneously respawn.
  if(dt>0.0 && random(i+u32(max(p.clock.y,0.0)*60.0)*2891336453u)<dt*8.0) {
   let r=vec3(random(i*3u),random(i*3u+1u),random(i*3u+2u))*2.0-1.0;
   c.position_age=vec4(p.source.xyz+r*p.source.w*0.5,0.0);
   c.velocity_density=vec4(r*0.3,1.0);
   c.thermal_pressure.w=p.wind_life.w*(0.5+random(i+137u));
  }
 } else {
  let flow=curl(c.position_age.xyz/max(p.source.w*2.0,0.1)+vec3(0,p.clock.y*0.15,0))*0.8
   -(c.position_age.xyz-p.source.xyz)*0.2+vec3(0,p.clock.z,0);
  c.velocity_density=vec4(mix(c.velocity_density.xyz,flow,1.0-exp(-3.0*dt)),1.0);
  c.position_age=vec4(c.position_age.xyz+c.velocity_density.xyz*dt,c.position_age.w+dt);
 }
 output[i]=c;
}
@compute @workgroup_size(64)
fn vertices(@builtin(global_invocation_id) gid: vec3<u32>) {
 let i=gid.x; if(i>=128u){return;} let c=input[i];
 let age=c.position_age.w/max(c.thermal_pressure.w,0.001);
 let fade=select(0.0,smoothstep(0.0,0.1,age)*(1.0-smoothstep(0.55,1.0,age)),c.thermal_pressure.w>0.0);
 quad(i,c.position_age.xyz,max(0.035,p.source.w*0.12)*(1.0-0.4*min(age,1.0)),fade*p.color.a);
}
