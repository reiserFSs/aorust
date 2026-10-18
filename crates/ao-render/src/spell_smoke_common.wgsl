// Adapted from ao-vk, Copyright (c) 2026 wannabuh, MIT; see THIRD_PARTY notices.
struct Params {
 source: vec4<f32>, color: vec4<f32>, uv: vec4<f32>, wind_life: vec4<f32>, clock: vec4<f32>,
 right: vec4<f32>, up: vec4<f32>, corner_size: vec4<f32>, bodies: array<vec4<f32>,16>,
 owners: array<vec4<u32>,4>, body_owners: array<vec4<u32>,2>, admission: vec4<u32>,
}
struct Cell { velocity_density: vec4<f32>, thermal_pressure: vec4<f32>, position_age: vec4<f32> }
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage,read> input: array<Cell>;
@group(0) @binding(2) var<storage,read_write> output: array<Cell>;
@group(0) @binding(3) var<storage,read_write> vb: array<f32>;
@group(0) @binding(4) var<storage,read> interaction_visibility: array<u32>;
fn owner_admitted(slot: u32) -> bool {
 if(slot==0u){return true;}
 if(slot>=arrayLength(&interaction_visibility)){return false;}
 return interaction_visibility[slot]!=0u;
}
fn history_admitted() -> bool {
 if(!owner_admitted(p.admission.x)){return false;}
 for(var i=0u;i<p.admission.y;i++){
  if(!owner_admitted(p.owners[i/4u][i%4u])){return false;}
 }
 return true;
}
@compute @workgroup_size(64)
fn sanitize(@builtin(global_invocation_id) gid:vec3<u32>) {
 let i=gid.x;if(i>=arrayLength(&input)){return;}
 var cell:Cell;
 if(history_admitted()){cell=input[i];}
 output[i]=cell;
}
fn hash(v: u32) -> u32 { var x=v; x ^= x>>16u; x *= 0x7feb352du; x ^= x>>15u; x *= 0x846ca68bu; return x^(x>>16u); }
fn random(v: u32) -> f32 { return f32(hash(v)>>8u)/16777216.0; }
fn quad(index: u32, center: vec3<f32>, extent: f32, alpha: f32) {
 let corners = array<vec2<f32>,6>(vec2(-1.0,-1.0),vec2(1.0,-1.0),vec2(-1.0,1.0),vec2(-1.0,1.0),vec2(1.0,-1.0),vec2(1.0,1.0));
 for(var k=0u;k<6u;k++) {
  let c=corners[k]; let pos=center+(p.right.xyz*c.x+p.up.xyz*c.y)*extent;
  let uv=p.uv.xy+(c*0.5+0.5)*p.uv.zw; let o=(index*6u+k)*12u;
  vb[o]=pos.x; vb[o+1u]=pos.y; vb[o+2u]=pos.z;
  vb[o+3u]=0.0; vb[o+4u]=1.0; vb[o+5u]=0.0;
  vb[o+6u]=uv.x; vb[o+7u]=uv.y;
  vb[o+8u]=p.color.r; vb[o+9u]=p.color.g; vb[o+10u]=p.color.b;
  vb[o+11u]=select(0.0,clamp(alpha,0.0,1.0),owner_admitted(p.admission.x));
 }
}
