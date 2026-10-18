// Stable-fluid stages adapted from ao-vk smoke_sim.comp; MIT (c) 2026 wannabuh.
// ponytail: fixed 24^3 source volumes, increase resolution only after GPU measurements.
const N=24; const COUNT=13824u;
fn coord(i:u32)->vec3<i32>{return vec3<i32>(i32(i%24u),i32((i/24u)%24u),i32(i/576u));}
fn index(c:vec3<i32>)->u32 {let q=clamp(c,vec3(0),vec3(N-1));return u32(q.x+q.y*N+q.z*N*N);}
fn vel(c:vec3<i32>)->vec3<f32>{return input[index(c)].velocity_density.xyz;}
fn press(c:vec3<i32>)->f32 {
 if(c.y<0){return input[index(c)].thermal_pressure.y;}
 if(any(c<vec3(0))||any(c>=vec3(N))){return 0.0;}
 return input[index(c)].thermal_pressure.y;
}
fn sample_cell(at:vec3<f32>)->Cell {
 var result:Cell;
 let base=vec3<i32>(floor(at));let f=fract(at);
 for(var k=0u;k<8u;k++){
  let offset=vec3<i32>(i32(k&1u),i32((k>>1u)&1u),i32((k>>2u)&1u));let q=base+offset;
  if(any(q<vec3(0))||any(q>=vec3(N))){continue;}
  let w=select(1.0-f,f,vec3<bool>(offset.x!=0,offset.y!=0,offset.z!=0));let weight=w.x*w.y*w.z;
  result.velocity_density+=input[index(q)].velocity_density*weight;
  result.thermal_pressure+=input[index(q)].thermal_pressure*weight;
 }
 return result;
}
@compute @workgroup_size(64)
fn advect(@builtin(global_invocation_id) gid:vec3<u32>){
 let i=gid.x;if(i>=COUNT){return;}let c=coord(i);let h=p.corner_size.w/f32(N);let dt=p.clock.x;
 let world=p.corner_size.xyz+(vec3<f32>(c)+0.5)*h;
 let shift=vec3(p.right.w,p.up.w,p.clock.w);
 let old=sample_cell(vec3<f32>(c)+shift/h);
 var cell=sample_cell(vec3<f32>(c)+shift/h-old.velocity_density.xyz*dt/h);
 var v=cell.velocity_density.xyz;
 v.y+=cell.thermal_pressure.x*1.8*dt;
 v+= (p.wind_life.xyz-v)*vec3(1.0-exp(-0.6*dt),0.0,1.0-exp(-0.6*dt));
 // Resolve sub-cell emitters into at least one cell, rather than losing their mass between samples.
 let r=(world-p.source.xyz)/max(p.source.w,h*0.75);
 let puff=0.65+0.2*sin(p.clock.y*2.1)+0.15*sin(p.clock.y*4.7+dot(p.source.xyz,vec3(0.17,0.31,0.13)));
 let feed=exp(-dot(r,r))*puff;
 v.y=mix(v.y,max(v.y,p.clock.z),feed*min(dt*60.0,1.0));
 for(var b=0u;b<8u;b++){
  let body=p.bodies[b];let motion=p.bodies[b+8u];if(motion.w<=0.0){continue;}
  let axis=vec3(body.x,clamp(world.y,body.y+motion.w,max(body.y+body.w-motion.w,body.y+motion.w)),body.z);
  let distance=length(world-axis);let inside=1.0-smoothstep(motion.w-h,motion.w+h,distance);
  v=mix(v,motion.xyz,inside*min(dt*60.0,1.0));
  let wake=(1.0-smoothstep(motion.w,motion.w*2.2+h,distance))*(1.0-inside)*length(motion.xyz);
  let seed=i/3u+u32(max(p.clock.y,0.0)*15.0)*131u+b;
  v+=(vec3(random(seed),random(seed+17u),random(seed+7919u))*2.0-1.0)*wake*dt;
 }
 if(c.y==0){v.y=max(v.y,0.0);}
 cell.velocity_density=vec4(v*exp(-0.05*dt),max(cell.velocity_density.w*exp(-dt/p.wind_life.w),feed*min(dt*8.0,1.0)));
 cell.thermal_pressure=vec4(max(cell.thermal_pressure.x*exp(-dt),feed*min(dt*8.0,1.0)),0.0,0.0,0.0);
 output[i]=cell;
}
@compute @workgroup_size(64)
fn divergence(@builtin(global_invocation_id) gid:vec3<u32>){
 let i=gid.x;if(i>=COUNT){return;}let c=coord(i);let h=p.corner_size.w/f32(N);var cell=input[i];
 cell.thermal_pressure.z=(vel(c+vec3(1,0,0)).x-vel(c-vec3(1,0,0)).x+vel(c+vec3(0,1,0)).y-select(vel(c-vec3(0,1,0)).y,-vel(c).y,c.y==0)+vel(c+vec3(0,0,1)).z-vel(c-vec3(0,0,1)).z)/(2.0*h);
 output[i]=cell;
}
@compute @workgroup_size(64)
fn jacobi(@builtin(global_invocation_id) gid:vec3<u32>){
 let i=gid.x;if(i>=COUNT){return;}let c=coord(i);let h=p.corner_size.w/f32(N);var cell=input[i];
 cell.thermal_pressure.y=(press(c+vec3(1,0,0))+press(c-vec3(1,0,0))+press(c+vec3(0,1,0))+press(c-vec3(0,1,0))+press(c+vec3(0,0,1))+press(c-vec3(0,0,1))-cell.thermal_pressure.z*h*h)/6.0;
 output[i]=cell;
}
@compute @workgroup_size(64)
fn project(@builtin(global_invocation_id) gid:vec3<u32>){
 let i=gid.x;if(i>=COUNT){return;}let c=coord(i);let h=p.corner_size.w/f32(N);var cell=input[i];
 cell.velocity_density=vec4(cell.velocity_density.xyz-vec3(press(c+vec3(1,0,0))-press(c-vec3(1,0,0)),press(c+vec3(0,1,0))-press(c-vec3(0,1,0)),press(c+vec3(0,0,1))-press(c-vec3(0,0,1)))/(2.0*h),cell.velocity_density.w);
 if(c.y==0){cell.velocity_density.y=max(cell.velocity_density.y,0.0);}
 output[i]=cell;
}
@compute @workgroup_size(64)
fn vertices(@builtin(global_invocation_id) gid:vec3<u32>){
 let i=gid.x;if(i>=COUNT){return;}let h=p.corner_size.w/f32(N);let c=coord(i);let density=input[i].velocity_density.w;
 // Beer-Lambert coverage of one grid cell; native source texture supplies the puff profile.
 let alpha=1.0-exp(-max(density,0.0)*h*3.2);
 quad(i,p.corner_size.xyz+(vec3<f32>(c)+0.5)*h,h*0.75,alpha);
}
@compute @workgroup_size(64)
fn curl_field(@builtin(global_invocation_id) gid:vec3<u32>){
 let i=gid.x;if(i>=COUNT){return;}let c=coord(i);let h=p.corner_size.w/f32(N);
 let dx=(vel(c+vec3(1,0,0))-vel(c-vec3(1,0,0)))/(2.0*h);
 let dy=(vel(c+vec3(0,1,0))-vel(c-vec3(0,1,0)))/(2.0*h);
 let dz=(vel(c+vec3(0,0,1))-vel(c-vec3(0,0,1)))/(2.0*h);
 let omega=vec3(dy.z-dz.y,dz.x-dx.z,dx.y-dy.x);var cell=input[i];
 cell.position_age=vec4(omega,length(omega));output[i]=cell;
}
@compute @workgroup_size(64)
fn confine(@builtin(global_invocation_id) gid:vec3<u32>){
 let i=gid.x;if(i>=COUNT){return;}let c=coord(i);let h=p.corner_size.w/f32(N);var cell=input[i];
 let gradient=vec3(input[index(c+vec3(1,0,0))].position_age.w-input[index(c-vec3(1,0,0))].position_age.w,
  input[index(c+vec3(0,1,0))].position_age.w-input[index(c-vec3(0,1,0))].position_age.w,
  input[index(c+vec3(0,0,1))].position_age.w-input[index(c-vec3(0,0,1))].position_age.w)/(2.0*h);
 let force=cross(gradient/max(length(gradient),0.00001),cell.position_age.xyz)*h*0.8*p.clock.x;
 cell.velocity_density=vec4(cell.velocity_density.xyz+force,cell.velocity_density.w);output[i]=cell;
}
