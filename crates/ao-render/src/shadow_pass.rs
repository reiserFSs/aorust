//! Encode admission first, then all three cascades using the same current pose.
use super::*;
use crate::sun_shadows::{CascadeFrame, SunShadows, CASCADES};

fn cascade_resolution(settings: EnhancedSettings) -> u32 {
    if !settings.sun_shadows && !settings.moon_shadows && !settings.volumetric_shafts { return 1; }
    let resolution = settings.shadow_map_size.clamp(256, 4096);
    if settings.volumetric_shafts && !settings.sun_shadows && !settings.moon_shadows {
        (resolution / 2).max(256)
    } else { resolution }
}

#[test]
fn shaft_only_maps_match_half_resolution_integration_without_lowering_surface_shadows() {
    let shafts = EnhancedSettings { volumetric_shafts:true, ..Default::default() };
    assert_eq!(cascade_resolution(EnhancedSettings::default()), 1);
    assert_eq!(cascade_resolution(shafts), 1024);
    assert_eq!(cascade_resolution(EnhancedSettings { sun_shadows:true, ..shafts }), 2048);
    assert_eq!(cascade_resolution(EnhancedSettings { moon_shadows:true, ..shafts }), 2048);
    assert_eq!(cascade_resolution(EnhancedSettings { shadow_map_size:256, ..shafts }), 256);
}

pub(super) fn authored_sun(scene: &Scene) -> Option<[f32; 3]> {
    scene.ground_light
        .filter(|(color, direction)| scene.sun_authored
            && color.iter().all(|c| c.is_finite()) && color.iter().any(|&c| c > 0.0)
            && direction.iter().all(|c| c.is_finite()) && Vec3::from(*direction).length_squared() > 1e-8)
        .map(|(_, direction)| direction)
}

/// Coalesce only consecutive commands; holes and draw order remain unchanged.
pub(super) fn push_static_shadow_command(commands: &mut Vec<(usize, std::ops::Range<u32>)>, draw: usize, instance: u32) {
    if let Some((last_draw, range)) = commands.last_mut() {
        if *last_draw == draw && range.end == instance {
            range.end += 1;
            return;
        }
    }
    commands.push((draw, instance..instance + 1));
}

impl Renderer {
    /// Developer/offscreen counts of admitted static instance commands and batched ranges per cascade.
    pub fn shadow_static_command_counts(&self) -> (usize, usize) {
        self.sun_shadows.as_ref().map_or((0, 0), |shadows| (
            shadows.static_commands.iter().map(|(_, range)| range.len()).sum(),
            shadows.static_commands.len(),
        ))
    }

    pub(super) fn update_shadow_sun(&mut self, scene: &Scene) {
        self.shadow_sun = authored_sun(scene);
        self.shadow_moon = crate::moon_shadows::source(scene);
    }

    /// Developer/offscreen only. Read each submitted sample before the next frame.
    pub fn set_shadow_gpu_timing(&mut self, enabled: bool) -> Result<()> {
        anyhow::ensure!(!enabled || self.effect_profiler.is_none(), "offline shadow timing and live per-effect profiling must be sampled separately");
        if enabled && self.gpu_timer.is_some() { return Err(anyhow!("frame and shadow GPU timing must be sampled separately")); }
        self.shadow_timer_pending = false;
        self.shadow_timer = if enabled { Some(FrameGpuTimer::new(&self.device, &self.queue)?) } else { None };
        Ok(())
    }

    pub fn read_shadow_gpu_ms(&mut self) -> Result<Option<f64>> {
        if !self.shadow_timer_pending { return Ok(None); }
        self.shadow_timer_pending = false;
        self.shadow_timer.as_mut().map(|timer| timer.read_ms(&self.device, &self.queue)).transpose()
    }

    pub fn shadow_resident_bytes(&self) -> u64 {
        self.sun_shadows.as_ref().map_or(0, SunShadows::resident_bytes)
    }

    fn shadow_timestamp(&mut self, last: bool) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        if let Some(timer) = &mut self.shadow_timer {
            if last {
                self.shadow_timer_pending = true;
                Some(timer.end_pass())
            } else if !timer.is_recording() {
                Some(timer.begin_pass(false))
            } else { None }
        } else {
            self.gpu_timer.as_mut().filter(|timer| !timer.is_recording()).map(|timer| timer.begin_pass(false))
        }
    }

    pub(crate) fn encode_enhanced_shadows(&mut self, encoder: &mut wgpu::CommandEncoder, cam: &Camera, size: (u32, u32), env: Environment, far: f32) {
        self.shadow_timer_pending = false;
        if !self.enhanced_settings.sun_shadows && !self.enhanced_settings.moon_shadows
            && !self.enhanced_settings.volumetric_shafts && !self.enhanced_settings.needs_admitted_lights()
            && self.enhanced_settings.night_mode != NightMode::Moonlit {
            self.sun_shadows = None;
            self.point_shadows = None;
            return;
        }
        let directional = self.enhanced_settings.sun_shadows || self.enhanced_settings.moon_shadows || self.enhanced_settings.volumetric_shafts;
        let admission = directional || self.enhanced_settings.needs_admitted_lights();
        // Moon illumination alone needs only the openness binding, not maps or a primary prepass.
        let resource_size = if admission { size } else { (1, 1) };
        let resolution = cascade_resolution(self.enhanced_settings);
        let mut shadows = self.sun_shadows.take().filter(|s| s.size == resource_size && s.resolution == resolution)
            .unwrap_or_else(|| SunShadows::new(self, resource_size, resolution));
        shadows.sun_replacement_active = false;
        shadows.native_fog_end = env.fog_end;
        let sun = (self.enhanced_settings.sun_shadows || self.enhanced_settings.volumetric_shafts)
            .then_some(self.shadow_sun).flatten().map(Vec3::from);
        let mut strength = sun.map_or(0.0, |direction| crate::sun_shadows::horizon_fade(direction.normalize().y))
            .min(Vec3::from(env.sun_dir).try_normalize().map_or(0.0, |direction| crate::sun_shadows::horizon_fade(direction.y)));
        if !env.sun_color.iter().any(|&channel| channel > 0.0) { strength = 0.0; }
        let moon = (self.enhanced_settings.moon_shadows || self.enhanced_settings.volumetric_shafts)
            .then(|| self.enhanced_moonlight()).flatten().filter(|_| strength <= 0.0);
        let direction = moon.map(|(direction, _)| Vec3::from(direction)).or(sun);
        if let Some((direction, _)) = moon { strength = crate::sun_shadows::horizon_fade(Vec3::from(direction).normalize().y); }
        let aspect = size.0 as f32 / size.1.max(1) as f32;
        // Fit is independent of local eye fog: only receiver reach changes.
        let shadow_far = self.lens.far.unwrap_or(180.0).min(180.0).max(self.lens.near + 0.01);
        let receiver_far = far.min(env.fog_end).min(shadow_far);
        let mut frame = direction.filter(|_| strength > 0.0).and_then(|direction|
            CascadeFrame::new(cam.pos, cam.forward(), cam.up(), self.lens.vertical_fov(aspect),
                aspect, self.lens.near, shadow_far, direction, resolution));
        if let Some(frame) = frame.as_mut() {
            frame.uniform.forward[3] = strength * self.enhanced_settings.shadow_strength.clamp(0.0, 1.0);
            frame.uniform.eye[3] = receiver_far;
            frame.uniform.control[1] = self.enhanced_settings.shadow_softness.clamp(0.0, 4.0).to_bits();
            if let Some((direction, budget)) = moon {
                frame.uniform.moon_direction = Vec3::from(direction).normalize().extend(1.0).to_array();
                frame.uniform.moon_budget = [budget[0], budget[1], budget[2], 0.0];
            }
        }
        if admission { self.prepare_actor_shadows(&mut shadows); } else { shadows.actor_ids.clear(); shadows.actor_groups.clear(); }
        if !shadows.actor_ids.is_empty() {
            let timestamp_writes = self.shadow_timestamp(false);
            let mut pass = encoder.begin_effect_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sun current-frame visible surface admission"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &shadows.primary_ids, resolve_target: None, depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &shadows.primary_depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }), stencil_ops: None,
                }), timestamp_writes, occlusion_query_set: None, multiview_mask: None,
            });
            pass.set_bind_group(0, &self.globals_bg, &[]);
            self.draw_static_shadows(&mut pass, &shadows, true);
            self.draw_actor_shadows(&mut pass, &shadows, true);
        }
        shadows.encode_actor_visibility(encoder);
        self.encode_point_shadows(encoder, cam, &mut shadows, [env.fog_start, env.fog_end, far]);
        let Some(mut frame) = frame else {
            shadows.update(&self.queue, &CascadeFrame { uniform: bytemuck::Zeroable::zeroed() });
            self.sun_shadows = Some(shadows);
            return;
        };
        frame.include_caster_depth(self.gpu.insts.iter().filter(|instance| !instance.hidden).map(|instance| (instance.center, instance.radius)));
        shadows.update(&self.queue, &frame);
        self.prepare_static_shadow_casters(&mut shadows, &frame);
        for layer in 0..CASCADES {
            let timestamp_writes = self.shadow_timestamp(layer == CASCADES - 1);
            let mut pass = encoder.begin_effect_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("current-frame sun cascade"), color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &shadows.layers[layer],
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }), stencil_ops: None,
                }), timestamp_writes, occlusion_query_set: None, multiview_mask: None,
            });
            pass.set_bind_group(0, &self.globals_bg, &[]);
            pass.set_bind_group(3, &shadows.cascade_groups[layer], &[]);
            // Static geometry does not need primary visibility admission. Dynamic
            // actors require at least one current-frame depth-tested visible pixel.
            self.draw_static_shadows(&mut pass, &shadows, false);
            self.draw_actor_shadows(&mut pass, &shadows, false);
            self.draw_canopy_leaf_shadows(&mut pass, &shadows);
        }
        shadows.sun_replacement_active = self.enhanced_settings.sun_shadows && moon.is_none() && frame.uniform.forward[3] > 0.0;
        self.sun_shadows = Some(shadows);
    }

    pub(crate) fn prepare_static_shadow_casters(&self, shadows: &mut SunShadows, frame: &CascadeFrame) {
        let planes = frame.uniform.matrices.map(|matrix| frustum_planes(&Mat4::from_cols_array_2d(&matrix)));
        shadows.static_commands.clear();
        let mut changed = shadows.static_transforms.len() != self.gpu.insts.len();
        if changed {
            shadows.static_transforms.clear();
            shadows.static_transforms.extend(self.gpu.insts.iter().map(|instance| instance.m));
        }
        for (index, draw) in self.gpu.opaque.iter().enumerate() {
            if draw.count == 0 { continue; }
            for slot in self.gpu.mesh_range[draw.mesh].clone() {
                let instance = &self.gpu.insts[slot];
                if instance.hidden || (!self.spaceships_shown && self.spaceship_slots[slot])
                    || self.far_away.iter().any(|run| run.slot == slot)
                    || self.movers.iter().any(|run| run.slot == slot) { continue; }
                let key = draw.node.map(|(node, _)| VisualKey::World(slot, node));
                if visual_priority(draw, key, &self.drawn_visuals).is_some_and(|priority| priority < 0) { continue; }
                if crate::shadow_coverage::admits_sphere(&planes, instance.center, instance.radius) {
                    if shadows.static_transforms[slot] != instance.m {
                        shadows.static_transforms[slot] = instance.m;
                        changed = true;
                    }
                    push_static_shadow_command(&mut shadows.static_commands, index, slot as u32);
                }
            }
        }
        if shadows.static_transforms.len() > shadows.static_capacity {
            shadows.static_capacity = shadows.static_transforms.len().next_power_of_two();
            shadows.static_instances = Some(self.device.create_buffer_tracked(&wgpu::BufferDescriptor {
                label: Some("bounded static shadow caster transforms"),
                size: (shadows.static_capacity * 64) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
            }));
            changed = true;
        }
        if changed {
            // ponytail: one bulk upload on static changes; use sparse ranges if animated statics become frequent.
            if let Some(buffer) = &shadows.static_instances {
                self.queue.write_buffer(buffer, 0, bytemuck::cast_slice(&shadows.static_transforms));
            }
        }
    }

    pub(crate) fn draw_static_shadows(&self, pass: &mut wgpu::RenderPass, shadows: &SunShadows, primary: bool) {
        let Some(instances) = (if primary { self.gpu.inst_bufs.get(self.frame) } else { shadows.static_instances.as_ref() }) else { return; };
        pass.set_vertex_buffer(1, instances.slice(..));
        let (mut pipe, mut mesh, mut mat) = (None, None, None);
        let mut draw_one = |pass: &mut wgpu::RenderPass, draw: &Draw, range: std::ops::Range<u32>| {
            let Some((vertices, indices)) = &self.gpu.meshes[draw.mesh] else { return; };
            if primary || pipe != Some(draw.pipe) {
                let Some(Some(pipeline)) = shadows.compiled.pipelines.get(&(false, draw.pipe, primary)) else { return; };
                pass.set_pipeline(pipeline);
                pipe = Some(draw.pipe);
            }
            if primary || mesh != Some(draw.mesh) {
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                mesh = Some(draw.mesh);
            }
            if primary || mat != Some(draw.mat) {
                pass.set_bind_group(1, &self.gpu.mats[draw.mat], &[]);
                mat = Some(draw.mat);
            }
            pass.draw_indexed(draw.first_index..draw.first_index + draw.count, 0, range);
        };
        if primary {
            for &(_, _, list, index, instance) in &self.sorted {
                let draw = &[&self.gpu.opaque, &self.gpu.blended, &self.gpu.liquid][list as usize][index];
                draw_one(pass, draw, instance..instance + 1);
            }
        } else {
            for (index, range) in &shadows.static_commands {
                draw_one(pass, &self.gpu.opaque[*index], range.clone());
            }
        }
        if primary {
            if let Some(ground) = &self.ground {
                for draw in &self.gpu.opaque {
                    if ground.materials[draw.mesh].is_some() && draw.count != 0 {
                        draw_one(pass, draw, self.vis_range[draw.mesh].clone());
                    }
                }
            }
        }
    }
}
