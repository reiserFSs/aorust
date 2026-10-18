//! Native liquid enhancement boundary: faithful water is always drawn unchanged.
//! Animated exact geometry supplies an optics mask; compositing cannot undo its opacity.
use crate::{water_dynamics::{WaterDynamics, WaterBody, SprayRecord, WakeSettings}, water_optics::{WaterOptics, WaterOpticsSwitches, WaterEligibility}, water_spray::WaterSpray, water_waves::{Shoreline, WaveOptions}, Camera, Environment, Mat4, Renderer, Vec3};

struct SurfaceRun {
    shoreline: Shoreline,
    sheets: crate::water_waves::WaterfallSheets,
    dynamics: Option<WaterDynamics>,
    shore: Vec<f32>,
    spacing: Vec<f32>,
    cell: Option<([i32; 4], bool, usize)>,
    vertices: Vec<ao_scene::Vertex>,
    indices: Vec<u32>,
    animated: Vec<[f32; 8]>,
}

#[derive(Default)]
pub(super) struct Water {
    wind: [f32; 3],
    camera_underwater: Option<bool>,
    surfaces: Vec<Option<SurfaceRun>>,
    static_lights: Vec<ao_scene::Light>,
    lights: Vec<ao_scene::Light>,
    optics: Option<WaterOptics>,
    bottom: Option<crate::water_bottom::WaterBottom>,
    spray: Option<WaterSpray>,
    positions: Vec<(u32, [f32; 3], f32, bool)>,
    previous: Vec<(u32, [f32; 3])>,
    history: Vec<(u32, [f32; 3])>,
    bodies: Vec<WaterBody>,
    geometry: Vec<[f32; 8]>,
    droplets: Vec<SprayRecord>,
    time: Option<f32>,
    prepared: bool,
}

impl Water {
    pub fn reset(&mut self) {
        let wind = self.wind;
        let camera_underwater = self.camera_underwater;
        let static_lights = std::mem::take(&mut self.static_lights);
        *self = Self { wind, camera_underwater, static_lights, ..Self::default() };
    }
    pub fn set_static_lights(&mut self, lights: &[ao_scene::Light]) {
        self.static_lights.clear();
        self.static_lights.extend_from_slice(lights);
    }
    pub fn target_bytes(&self) -> u64 {
        self.optics.as_ref().map_or(0, WaterOptics::target_bytes)
    }
    pub fn is_ready(&self) -> bool {
        self.optics.as_ref().is_none_or(WaterOptics::is_ready)
            && self.spray.as_ref().is_none_or(WaterSpray::is_ready)
            && self.bottom.as_ref().is_none_or(crate::water_bottom::WaterBottom::is_ready)
    }
    pub fn wait(&self) {
        if let Some(optics) = &self.optics { optics.wait(); }
        if let Some(spray) = &self.spray { spray.wait(); }
        if let Some(bottom) = &self.bottom { bottom.wait(); }
    }
}

impl Renderer {
    pub(crate) fn set_camera_underwater(&mut self, membership: Option<bool>) {
        self.water.camera_underwater = membership;
    }
    pub(crate) fn set_water_wind(&mut self, wind: [f32; 3]) {
        self.water.wind = wind.map(|x| if x.is_finite() { x } else { 0.0 });
    }

    /// Queue effect programs at option/scene publication, outside the world frame guard.
    pub(super) fn configure_water(&mut self) {
        if !self.enhanced_settings.water_enabled() {
            self.water.reset();
            return;
        }
        if !self.liquids.iter().any(|run| matches!(run.source.style, 0 | 4)) { return; }
        if self.water.optics.is_none() {
            let mut optics = WaterOptics::new(&self.device, self.format);
            optics.set_surfaces(&self.device, self.liquids.iter().map(|run| &run.source));
            self.water.optics = Some(optics);
        }
        if self.enhanced_settings.waterfalls && self.water.spray.is_none() {
            self.water.spray = Some(WaterSpray::new(&self.device, self.format));
        }
        if (self.enhanced_settings.water_refraction || self.enhanced_settings.water_caustics || self.enhanced_settings.water_foam)
            && self.water.bottom.is_none() {
            self.water.bottom = Some(crate::water_bottom::WaterBottom::new(&self.device, &self.g_layout, &self.tex_layout));
        }
    }

    pub(super) fn prepare_water(&mut self, camera: &Camera, vp: Mat4, size: (u32, u32), env: Environment) {
        let settings = self.enhanced_settings;
        let switches = WaterOpticsSwitches {
            waves: settings.water_waves, wakes: settings.water_wakes,
            ripples: settings.water_ripples, refraction: settings.water_refraction,
            reflections: settings.water_reflections, foam: settings.water_foam,
            waterfalls: settings.waterfalls, underwater: settings.underwater,
            caustics: settings.water_caustics, sun_glint: settings.water_sun_glint,
            realism: settings.water_realism,
        };
        if !settings.water_enabled() || !self.liquids.iter().any(|run| matches!(run.source.style, 0 | 4)) {
            self.water.reset();
            return;
        }
        if self.water.surfaces.is_empty() {
            self.water.surfaces = self.liquids.iter().map(|run| matches!(run.source.style, 0 | 4).then(|| SurfaceRun {
                shoreline: Shoreline::new_with_ground(&run.source, self.ground.as_ref().map_or(&[], |ground| ground.detail.patches())),
                dynamics: None, shore: Vec::new(), cell: None,
                sheets: crate::water_waves::WaterfallSheets::new(&run.source),
                vertices: Vec::new(), indices: Vec::new(), spacing: Vec::new(), animated: Vec::new(),
            })).collect();
        }
        let mut positions = std::mem::take(&mut self.water.positions);
        if settings.water_wakes { self.enhanced_water_contacts(env.fog_end, &mut positions); } else { positions.clear(); }
        let mut history = std::mem::take(&mut self.water.history);
        if settings.water_wakes { self.enhanced_water_body_positions(env.fog_end, &mut history); } else { history.clear(); }
        let water = &mut self.water;
        let elapsed = water.time.map_or(0.0, |time| (self.time - time).max(0.0));
        let dt = elapsed.min(0.1);
        water.bodies.clear();
        for &(id, position, surface, swimming) in &positions {
            let previous_position = water.previous.iter().find(|(old, _)| *old == id).map_or(position, |(_, p)| *p);
            let velocity = if elapsed > 0.0 { ((Vec3::from_array(position) - Vec3::from_array(previous_position)) / elapsed).to_array() } else { [0.0; 3] };
            water.bodies.push(WaterBody { id: u64::from(id), position, previous_position, velocity, surface, swimming });
        }
        std::mem::swap(&mut water.previous, &mut history);
        water.history = history;
        water.positions = positions;
        water.time = Some(self.time);
        water.geometry.clear();
        water.droplets.clear();
        let cell = ao_scene::liquid::Surface::cell(camera.pos.to_array());
        for (run, surface) in self.liquids.iter().zip(&mut water.surfaces) {
            let Some(surface) = surface else { continue; };
            let key = ([cell[0], cell[1], (camera.pos.x / 8.0).floor() as i32, (camera.pos.z / 8.0).floor() as i32], self.realistic_water, run.vertices.len());
            if surface.cell != Some(key) {
                crate::water_waves::refine(&run.vertices, &run.indices, camera.pos.to_array(), &mut surface.vertices, &mut surface.indices);
                surface.shoreline.distances(&surface.vertices, &mut surface.shore);
                for (vertex, shore) in surface.vertices.iter().zip(&mut surface.shore) {
                    if crate::water_waves::is_waterfall(vertex.normal) { *shore = -(surface.sheets.along(vertex.pos) + 1.0); }
                }
                surface.spacing.clear();
                surface.spacing.resize(surface.vertices.len(), 0.0);
                for triangle in surface.indices.as_chunks::<3>().0 {
                    for edge in 0..3 {
                        let a = triangle[edge] as usize;
                        let b = triangle[(edge + 1) % 3] as usize;
                        let length = Vec3::from_array(surface.vertices[a].pos).distance(Vec3::from_array(surface.vertices[b].pos));
                        surface.spacing[a] = surface.spacing[a].max(length);
                        surface.spacing[b] = surface.spacing[b].max(length);
                    }
                }
                surface.cell = Some(key);
            }
            if settings.water_wakes || settings.waterfalls {
                let dynamics = surface.dynamics.get_or_insert_with(|| WaterDynamics::new(&run.source));
                dynamics.set_settings(WakeSettings { strength: settings.water_wake_strength, visibility: settings.water_wake_visibility,
                    duration: settings.water_wake_duration, speed: settings.water_wake_speed,
                    treading: settings.water_wake_treading, splash: settings.water_wake_splash });
                dynamics.update(camera.pos.to_array(), dt, &water.bodies, settings.water_wakes, settings.waterfalls);
                water.droplets.extend_from_slice(dynamics.spray());
            } else { surface.dynamics = None; }
            surface.animated.clear();
            for ((vertex, &shore), &spacing) in surface.vertices.iter().zip(&surface.shore).zip(&surface.spacing) {
                let wave = crate::water_waves::animate(vertex.pos, vertex.normal, self.time,
                    [water.wind[0], water.wind[2]], shore, WaveOptions {
                        waves: settings.water_waves, ripples: settings.water_ripples,
                        spacing,
                        view_distance: Vec3::from_array(vertex.pos).distance(camera.pos), ..Default::default()
                    });
                let mut position = Vec3::from_array(wave.position);
                let mut normal = Vec3::from_array(wave.normal);
                let mut foam = ((1.0 - wave.fold) * 3.0).clamp(0.0, 1.0);
                if let Some(dynamics) = &surface.dynamics {
                    let sample = dynamics.sample(vertex.pos);
                    position.y += sample.displacement.max(0.0) * (shore / 1.5).clamp(0.0, 1.0);
                    normal += Vec3::from_array(sample.normal) - Vec3::Y;
                    foam = foam.max(sample.foam);
                }
                let normal = normal.normalize_or_zero();
                surface.animated.push([position.x, position.y, position.z, normal.x, normal.y, normal.z, shore, foam]);
            }
            water.geometry.extend(surface.indices.iter().map(|&i| surface.animated[i as usize]));
        }
        water.lights.clear();
        if settings.water_sun_glint || settings.waterfalls {
            water.lights.extend(water.static_lights.iter().filter(|light| light.zone.is_none_or(|zone| {
                self.lod.as_ref().and_then(|lod| lod.levels.get(zone as usize))
                    .is_some_and(|&level| ao_scene::StatelLod::lights_active(level))
            })).copied());
            water.lights.sort_unstable_by(|a, b| {
                Vec3::from_array(a.pos).distance_squared(camera.pos)
                    .total_cmp(&Vec3::from_array(b.pos).distance_squared(camera.pos))
            });
            water.lights.truncate(64);
        }
        let optics = water.optics.as_mut().expect("water programs queued at scene/options publication");
        optics.set_geometry(&self.device, &self.queue, &water.geometry);
        optics.set_lights(&self.device, &self.queue, &water.lights);
        optics.set_membership(water.camera_underwater);
        water.prepared = optics.prepare(&self.device, &self.queue, size, vp, vp.inverse(), camera.pos, self.time,
            WaterEligibility { daylight: crate::grade::daylight_weight(self.sky.pose.daylight) > 0.0,
                outdoors: self.sky.pose.daylight.is_some(), fog_start: env.fog_start, fog_end: env.fog_end,
                samples: self.world.msaa,
                sun_direction: env.sun_dir, sun_colour: env.sun_color, sky_colour: env.sky_color, fog_colour: env.fog_color }, switches);
        if settings.waterfalls {
            let spray = water.spray.as_mut().expect("waterfall programs queued at options publication");
            spray.prepare(&self.device, &self.queue, size, vp, camera.pos, self.time,
                [env.fog_start, env.fog_end, 1.0, 0.0],
                std::array::from_fn(|i| crate::gamma((env.ambient[i] + env.sun_color[i] * 0.25).clamp(0.0, 1.0))),
                &water.droplets);
        } else { water.spray = None; }
    }

    pub(super) fn encode_water_bottom(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(depth) = self.water.optics.as_ref().and_then(WaterOptics::bottom_depth) else { return; };
        let timestamps = self.gpu_timer.as_mut().and_then(|timer| {
            if timer.is_recording() { None } else { Some(timer.begin_pass(false)) }
        });
        self.water.bottom.as_ref().expect("bottom programs queued for depth-dependent optics")
            .encode(encoder, depth, &self.globals_bg, &self.gpu, &self.vis_range, self.frame, &self.world.pipes, self.ground.as_ref(), timestamps);
    }

    pub(super) fn encode_water(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(frame) = &self.enhanced_frame else { return; };
        if self.water.prepared {
            self.water.optics.as_mut().expect("prepared water optics").encode(&self.device, encoder, &frame.colour, &frame.resolved_depth);
        }
        if let Some(spray) = &mut self.water.spray { spray.encode(encoder, &frame.colour, &frame.resolved_depth); }
    }
}
