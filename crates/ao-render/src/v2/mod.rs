//! Enhanced v2: one designed deferred renderer next to the untouched faithful path (docs: `~/aomac-handoff/v2/DESIGN.md`).
//!
//! `Renderer::render` takes exactly one branch into [`Renderer::render_v2`] when `EnhancedSettings::enhanced_v2` is set;
//! nothing below that branch runs for Original / Classic+. Frame graph (High): [shadows] -> G-buffer -> [AO] -> resolve
//! -> FX (blended draws) -> post. Everything here reads only the admitted draw lists the faithful path builds.
mod ao;
mod bloom;
mod consts;
mod gbuffer;
mod post;
mod pyramid;
mod resolve;
mod taa;
mod timers;

/// G2 (actor motion) format, for the actor motion pass nested in `actors`.
pub(crate) const G2_FORMAT: wgpu::TextureFormat = gbuffer::G2;

use crate::{Camera, Renderer, Targets};
use consts::FrameConsts;
use glam::{Mat4, Vec3};

/// Size-dependent resources.
struct Sized {
    targets: gbuffer::Targets,
    resolve_bind: wgpu::BindGroup,
    pyramid: pyramid::Pyramid,
    ao: ao::Ao,
    bloom: bloom::BloomTargets,
    taa: taa::TaaTargets,
    /// `post_binds[k]` reads the TAA output of frame parity `k` (`taa.hist[1 - k]`).
    post_binds: [wgpu::BindGroup; 2],
}

pub(crate) struct V2 {
    pipes: gbuffer::Pipes,
    pyramid: pyramid::Pipes,
    ao: ao::Pipes,
    resolve: resolve::Resolve,
    bloom: bloom::Bloom,
    taa_pass: taa::Taa,
    motion: crate::actors::v2_motion::ActorMotion,
    post: post::Post,
    consts: wgpu::Buffer,
    /// `post::Ctl`, rewritten every frame.
    ctl: wgpu::Buffer,
    sized: Option<Sized>,
    timers: Option<timers::PassTimers>,
    frame: u32,
    prev_view_proj: Mat4,
    /// Sub-pixel projection jitter + history accumulation (TAA). `AOMAC_V2_NO_TAA=1` (harness diagnostic, read once) turns the
    /// accumulation and the jitter off for before/after stills.
    taa: bool,
    /// What the TAA history was rendered under (`None` = no history: first frame or resize).
    taa_key: Option<taa::HistoryKey>,
    /// Frame parity of the TAA ping-pong.
    taa_index: usize,
}

impl V2 {
    pub(crate) fn new(r: &Renderer) -> Self {
        Self {
            pipes: gbuffer::Pipes::new(r),
            pyramid: pyramid::Pipes::new(&r.device),
            ao: ao::Pipes::new(&r.device),
            resolve: resolve::Resolve::new(&r.device, &[]),
            bloom: bloom::Bloom::new(&r.device),
            taa_pass: taa::Taa::new(&r.device),
            motion: crate::actors::v2_motion::ActorMotion::new(r),
            post: post::Post::new(&r.device, r.format),
            consts: r.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("v2 frame constants"), size: std::mem::size_of::<FrameConsts>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
            }),
            ctl: r.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("v2 post controls"), size: std::mem::size_of::<post::Ctl>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
            }),
            sized: None,
            timers: None,
            frame: 0,
            prev_view_proj: Mat4::IDENTITY,
            taa: std::env::var_os("AOMAC_V2_NO_TAA").is_none(),
            taa_key: None,
            taa_index: 0,
        }
    }
}

/// Day/dusk/night lighting handed to the resolve, derived from the zone environment (authored values, pushed).
struct Look {
    sun: Vec3,
    sun_up: f32,
    sun_color: Vec3,
    sky_ambient: Vec3,
    ground_ambient: Vec3,
}

impl Look {
    fn from_env(env: &ao_scene::Environment) -> Self {
        let sun = Vec3::from(env.sun_dir).normalize_or_zero();
        let up = crate::sun_shadows::horizon_fade(sun.y);
        let ambient = Vec3::from(env.ambient);
        Self {
            sun,
            sun_up: if up > 0.0 { 1.0 } else { 0.0 },
            // Warmer and brighter key than authored (DESIGN 4.2), cool shade fill, warm ground bounce.
            sun_color: Vec3::from(env.sun_color) * 1.2 * up,
            sky_ambient: ambient * Vec3::new(0.72, 0.91, 1.2),
            ground_ambient: ambient * Vec3::new(1.1, 0.8, 0.5),
        }
    }
}

impl Renderer {
    /// v2 replaces the world pass only; GUI viewports, depth-clear layers and other native screens stay faithful.
    pub(crate) fn v2_eligible(&self) -> bool {
        self.screen.layers.is_empty() && self.screen.depth_clears.is_empty()
    }

    /// Per-pass GPU timing for the offline harness (allocates timestamp queries; off by default).
    pub fn set_v2_pass_timing(&mut self, enabled: bool) -> anyhow::Result<()> {
        if self.v2.is_none() {
            let v2 = V2::new(self);
            self.v2 = Some(v2);
        }
        let timers = enabled.then(|| timers::PassTimers::new(&self.device, &self.queue)).flatten();
        anyhow::ensure!(timers.is_some() || !enabled, "v2 pass timing needs TIMESTAMP_QUERY");
        self.v2.as_mut().expect("created above").timers = timers;
        Ok(())
    }

    /// Per-pass GPU milliseconds of the last submitted v2 frame (blocks until it completes). Empty without timing.
    pub fn v2_pass_ms(&mut self, lag: usize) -> anyhow::Result<Vec<(&'static str, f64)>> {
        self.v2.as_ref().and_then(|v2| v2.timers.as_ref()).map_or(Ok(Vec::new()), |timers| timers.read(&self.device, lag))
    }

    pub(crate) fn render_v2(&mut self, resolve: &wgpu::TextureView, t: &Targets, cam: &Camera) -> wgpu::SubmissionIndex {
        let _compile_scope = crate::pipeline_creation::FrameGuard::enter();
        let mut v2 = self.v2.take().unwrap_or_else(|| V2::new(self));
        self.update_liquids(cam.pos);
        self.update_ground_detail(cam.pos);
        self.update_lod(cam.pos.x, cam.pos.z);
        self.step_movers();
        self.pose_far_away(cam.pos);
        let mut env = self.env;
        let far = self.lens.far.unwrap_or_else(|| (env.fog_end * 1.1).max(50.0));
        if self.screen.fog_disabled { env.fog_start = f32::MAX; env.fog_end = f32::MAX; }
        if let Some(model) = self.fog.as_mut().filter(|_| !self.screen.fog_disabled) {
            crate::apply_camera_fog(&mut env, model, self.lens, far, cam.pos.to_array(), self.fog_density_scale);
        }
        let size = t.size;
        let aspect = size.0 as f32 / size.1.max(1) as f32;
        let proj = Mat4::perspective_rh(self.lens.vertical_fov(aspect), aspect, self.lens.near, far);
        let view = Mat4::look_to_rh(cam.pos, cam.forward(), cam.up());
        let jitter = if v2.taa { consts::halton_jitter(v2.frame) } else { [0.0, 0.0] };
        let view_proj = FrameConsts::jittered(proj, jitter, size) * view;
        let view_proj_unjit = proj * view;

        let w = ao_scene::wave_curves(self.time);
        let globals = self.frame_globals(cam, &env, view_proj, far, &w);
        self.queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));
        self.cull_frame(cam, view_proj);
        self.sort_draws(cam.pos);

        if v2.sized.as_ref().is_none_or(|s| s.targets.size != size) {
            let targets = gbuffer::Targets::new(&self.device, size);
            let pyramid = pyramid::Pyramid::new(&self.device, &v2.pyramid, &v2.consts, &targets);
            let ao = ao::Ao::new(&self.device, &v2.ao, &v2.consts, &pyramid, &targets);
            let resolve_bind = v2.resolve.bind(&self.device, &v2.consts, &targets, Some(ao.view()));
            let bloom = v2.bloom.bind(&self.device, &v2.consts, &v2.ctl, &targets);
            let taa = v2.taa_pass.bind(&self.device, &v2.consts, &v2.ctl, &bloom.comp, &targets);
            let post_binds = [0, 1].map(|k| v2.post.bind(&self.device, &v2.consts, &v2.ctl, &taa.hist[1 - k], &targets.depth));
            v2.sized = Some(Sized { targets, resolve_bind, pyramid, ao, bloom, taa, post_binds });
            v2.taa_key = None;
            v2.taa_index = 0;
        }
        let sized = v2.sized.as_ref().expect("sized above");
        let look = Look::from_env(&env);
        // Moon A: the shadowed night key, lit by the authored moon pose; colour per DESIGN 4.2 (art tuning, not authored data).
        let moon = self.shadow_moon.and_then(|m| {
            let budget = Vec3::from(m.world_gamma_budget?);
            Some((Vec3::from(m.toward_light).normalize_or_zero(), Vec3::new(0.235, 0.39, 0.55) * 0.4 * (budget.max_element() / 0.2).clamp(0.0, 1.0)))
        });
        // Order-independent hash of the admitted actor ids; temporal consumers reset when it changes (DESIGN section 8).
        let admission = self.admitted_actor_ids().fold(0u32, |hash, id| hash ^ id.wrapping_mul(0x9E37_79B9).rotate_left(13));
        // TAA history is discarded on frame 0 / resize, camera cuts and any change of the admitted-actor set (a hidden actor cannot
        // ghost); every actor pose of the motion pass shares that decision.
        let key = taa::HistoryKey { admission, eye: cam.pos, forward: cam.forward() };
        let history_reset = !v2.taa || taa::history_reset(v2.taa_key.as_ref(), &key);
        v2.taa_key = Some(key);
        let previous_unjit = if v2.frame == 0 { view_proj_unjit } else { v2.prev_view_proj };
        let consts = FrameConsts {
            view: view.to_cols_array_2d(),
            view_proj: view_proj.to_cols_array_2d(),
            inv_view_proj: view_proj.inverse().to_cols_array_2d(),
            view_proj_unjit: view_proj_unjit.to_cols_array_2d(),
            prev_view_proj_unjit: previous_unjit.to_cols_array_2d(),
            eye: cam.pos.extend(far).to_array(),
            viewport: [size.0 as f32, size.1 as f32, 1.0 / size.0 as f32, 1.0 / size.1 as f32],
            proj: [self.lens.near, far, (0.5 * self.lens.vertical_fov(aspect)).tan(), aspect],
            jitter: [jitter[0], jitter[1], v2.frame as f32, self.time],
            sun_dir: look.sun.extend(look.sun_up).to_array(),
            sun_color: look.sun_color.extend(0.0).to_array(),
            moon_dir: moon.map_or([0.0; 4], |(dir, _)| dir.extend(1.0).to_array()),
            moon_color: moon.map_or([0.0; 4], |(_, color)| color.extend(0.0).to_array()),
            sky_ambient: look.sky_ambient.extend(0.0).to_array(),
            ground_ambient: look.ground_ambient.extend(0.0).to_array(),
            fog_color: Vec3::from(env.fog_color).extend(0.0).to_array(),
            fog: [env.fog_start, env.fog_end, 0.0, 0.0],
            grade: [1.0, 1.08, 1.05, 0.2],
            flags: [v2.frame, 1, admission, 0],
        };
        self.queue.write_buffer(&v2.consts, 0, bytemuck::bytes_of(&consts));
        let post_ctl = post::Ctl { bloom: self.enhanced_settings.bloom_strength.clamp(0.0, 1.0), cas: self.enhanced_settings.cas_strength.clamp(0.0, 1.0),
            pad: [0.0; 2], taa: [u32::from(history_reset), 0, 0, 0] };
        self.queue.write_buffer(&v2.ctl, 0, bytemuck::bytes_of(&post_ctl));
        v2.motion.prepare(self, view_proj_unjit, previous_unjit, history_reset);

        let mut enc = self.device.create_command_encoder(&Default::default());
        if let Some(timers) = &v2.timers { timers.begin_frame(); }
        self.encode_actor_skins(&mut enc);
        if let Some(buf) = self.gpu.inst_bufs.get(self.frame) {
            if !self.vis.is_empty() { self.queue.write_buffer(buf, 0, bytemuck::cast_slice(&self.vis)); }
        }
        let grid = self.update_sky_frame(cam, &env, far, &w);

        let timer = v2.timers.as_ref();
        let targets = &sized.targets;
        let sky_clear = crate::framebuffer_clear(env.sky_color);
        let mut calls = 0;
        {
            let color = |view, clear| Some(wgpu::RenderPassColorAttachment { view, resolve_target: None, depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Store } });
            let sky = wgpu::Color { r: sky_clear.r.powf(2.2), g: sky_clear.g.powf(2.2), b: sky_clear.b.powf(2.2), a: 1.0 };
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("v2 G-buffer"),
                color_attachments: &[color(&targets.sky, sky), color(&targets.g0, wgpu::Color::TRANSPARENT), color(&targets.g1, wgpu::Color::TRANSPARENT),
                    color(&targets.g2, wgpu::Color::TRANSPARENT), color(&targets.g3, wgpu::Color::TRANSPARENT)],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &targets.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }), stencil_ops: None }),
                timestamp_writes: timer.and_then(|t| t.render("gbuffer")),
                occlusion_query_set: None, multiview_mask: None,
            });
            calls += self.v2_draw_g(&mut pass, &v2.pipes.g, grid);
        }
        // [slot 1] actor admission (ids pass + reduction against the G-buffer depth), then [slot 2] shadow maps (sun cascades static
        // + dynamic layer, moon A, lamp cubes; casters from admitted actors only), [slot 3] pyramid + AO (writes the AO texture the
        // resolve reads, flags.y bit 0), [slot 4] light clusters: all encoded here, before the resolve.
        sized.pyramid.encode(&v2.pyramid, &mut enc, timer.and_then(|t| t.compute("hiz")));
        sized.ao.encode(&v2.ao, &self.queue, &mut enc, timer, admission, jitter != [0.0, 0.0]);
        v2.resolve.encode(&mut enc, &sized.resolve_bind, &[], size, timer.and_then(|t| t.compute("resolve")));
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("v2 FX"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &targets.hdr, resolve_target: None, depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &targets.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }), stencil_ops: None }),
                timestamp_writes: timer.and_then(|t| t.render("fx")),
                occlusion_query_set: None, multiview_mask: None,
            });
            calls += self.v2_draw_fx(&mut pass, &v2.pipes.fx);
        }
        // [slot 5a] actor motion into G2, then bloom (seeded by G3 only; writes HDR + halo), [slot 5b] TAA, then post (CAS + grade).
        v2.motion.encode(self, &mut enc, &targets.depth, &targets.g2, timer.and_then(|t| t.render("motion")));
        v2.bloom.encode(&mut enc, &sized.bloom, timer.and_then(|t| t.compute("bloom")));
        v2.taa_pass.encode(&mut enc, &sized.taa, v2.taa_index, size, timer.and_then(|t| t.compute("taa")));
        v2.post.encode(&mut enc, &sized.post_binds[v2.taa_index], resolve, timer.and_then(|t| t.render("post")));
        v2.motion.finish(self, &mut enc);
        v2.taa_index ^= 1;
        if let Some(timers) = &v2.timers { timers.tail(&mut enc); timers.resolve(&mut enc); }

        self.drawn_visuals.extend(self.pending_visuals.get_mut().drain());
        self.stats = crate::FrameStats { instances: self.vis.len(), draw_calls: calls };
        v2.prev_view_proj = view_proj_unjit;
        v2.frame = v2.frame.wrapping_add(1);
        let submission = self.queue.submit([enc.finish()]);
        if let Some(timers) = &v2.timers { timers.submitted(&submission); }
        self.v2 = Some(v2);
        submission
    }
}
