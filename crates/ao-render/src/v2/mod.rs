//! Enhanced v2: one designed deferred renderer next to the untouched faithful path (docs: `~/aomac-handoff/v2/DESIGN.md`).
//!
//! `Renderer::render` takes exactly one branch into [`Renderer::render_v2`] when `EnhancedSettings::enhanced_v2` is set;
//! nothing below that branch runs for Original / Classic+. Frame graph (High): [shadows] -> G-buffer -> [AO] -> resolve
//! -> FX (blended draws) -> post. Everything here reads only the admitted draw lists the faithful path builds.
mod ao;
mod bloom;
mod admission;
mod consts;
mod gbuffer;
mod lamps;
mod post;
mod pyramid;
mod resolve;
mod shadows;
mod region;
mod skyfix;
mod taa;
mod timers;

/// G2 (actor motion) format, for the actor motion pass nested in `actors`.
pub(crate) const G2_FORMAT: wgpu::TextureFormat = gbuffer::G2;
pub(crate) use admission::item_slots;

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
    skyfix_bind: wgpu::BindGroup,
}

pub(crate) struct V2 {
    pipes: gbuffer::Pipes,
    pyramid: pyramid::Pipes,
    ao: ao::Pipes,
    resolve: resolve::Resolve,
    bloom: bloom::Bloom,
    taa_pass: taa::Taa,
    skyfix: skyfix::SkyFix,
    motion: crate::actors::v2_motion::ActorMotion,
    post: post::Post,
    admission: admission::Admission,
    shadows: shadows::Shadows,
    consts: wgpu::Buffer,
    /// `post::Ctl`, rewritten every frame.
    ctl: wgpu::Buffer,
    lamps: lamps::Lamps,
    sized: Option<Sized>,
    timers: Option<timers::PassTimers>,
    /// `timers` was created for the live overlay/slow-frame profiler (not the harness), so it is dropped when that profiler stops.
    live_timers: bool,
    frame: u32,
    prev_view_proj: Mat4,
    /// Sub-pixel projection jitter + history accumulation (TAA). `AOMAC_V2_NO_TAA=1` (harness diagnostic, read once) turns the
    /// accumulation and the jitter off for before/after stills.
    taa: bool,
    /// What the TAA history was rendered under (`None` = no history: first frame or resize).
    taa_key: Option<taa::HistoryKey>,
    /// Frame parity of the TAA ping-pong.
    taa_index: usize,
    /// Previous frame's drawn items: the diff is this frame's admission-changed screen regions.
    region: region::Tracker,
    /// `region::RectUniform`, rewritten every frame; read by TAA (binding 10) and the AO denoise (binding 9).
    rects: wgpu::Buffer,
}

/// Owned handles [`V2::new`] needs from the renderer, so it can run off the render thread.
pub(crate) struct Seed {
    pub(crate) device: wgpu::Device,
    format: wgpu::TextureFormat,
    pub(crate) g_layout: wgpu::BindGroupLayout,
    pub(crate) tex_layout: wgpu::BindGroupLayout,
    holo_layout: wgpu::BindGroupLayout,
    shadow_pipelines: std::sync::Arc<crate::sun_shadows::Compiled>,
}

impl V2 {
    pub(crate) fn new(r: &Seed) -> Self {
        let admission = admission::Admission::new(r);
        let shadows = shadows::Shadows::new(r, admission.visibility());
        let lamps = lamps::Lamps::new(&r.device, admission.visibility());
        let plugins = [
            shadows.plugin(),
            lamps.plugin(),
        ];
        Self {
            pipes: gbuffer::Pipes::new(r),
            pyramid: pyramid::Pipes::new(&r.device),
            ao: ao::Pipes::new(&r.device),
            resolve: resolve::Resolve::new(&r.device, &plugins),
            bloom: bloom::Bloom::new(&r.device),
            taa_pass: taa::Taa::new(&r.device),
            skyfix: skyfix::SkyFix::new(&r.device),
            motion: crate::actors::v2_motion::ActorMotion::new(r),
            post: post::Post::new(&r.device, r.format),
            admission,
            shadows,
            consts: r.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("v2 frame constants"), size: std::mem::size_of::<FrameConsts>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
            }),
            ctl: r.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("v2 post controls"), size: std::mem::size_of::<post::Ctl>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
            }),
            lamps,
            sized: None,
            timers: None,
            live_timers: false,
            frame: 0,
            prev_view_proj: Mat4::IDENTITY,
            taa: std::env::var_os("AOMAC_V2_NO_TAA").is_none(),
            taa_key: None,
            taa_index: 0,
            region: region::Tracker::default(),
            rects: r.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("v2 admission regions"), size: std::mem::size_of::<region::RectUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
            }),
        }
    }
}

/// Harness diagnostic, read once: `AOMAC_V2_OFF=ao,bloom,cas,cube,post2,admission,shadows` switches optional passes off so the `frame` p50
/// with/without gives their marginal cost (images are not meaningful while a pass is off).
pub(super) fn off(name: &str) -> bool {
    static LIST: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    LIST.get_or_init(|| std::env::var("AOMAC_V2_OFF").unwrap_or_default()).split(',').any(|n| n == name)
}

/// Day/dusk/night lighting handed to the resolve, derived from the zone environment (authored values, pushed).
struct Look {
    sun: Vec3,
    sun_up: f32,
    sun_color: Vec3,
    sky_ambient: Vec3,
    ground_ambient: Vec3,
    native_ambient_packed: u32,
}

impl Look {
    fn from_env(env: &ao_scene::Environment) -> Self {
        let sun = Vec3::from(env.sun_dir).normalize_or_zero();
        let up = crate::sun_shadows::horizon_fade(sun.y);
        let ambient = Vec3::from(env.ambient);
        Self {
            sun,
            sun_up: if up > 0.0 { 1.0 } else { 0.0 },
            // Warmer and brighter key than authored (DESIGN 4.2). The hemisphere fill keeps the AUTHORED ambient hue: a blue/orange tint on
            // it rotated every shaded surface (ICC platform hull +12 deg from Original); only its brightness is lifted (x1.4 sky, x1.3 ground).
            sun_color: Vec3::from(env.sun_color) * 1.2 * up,
            // The shade floor: the native device ambient (resolve.wgsl `light += max(ambient * ao, floor)`), 3 x 10 bit in flags.w.
            native_ambient_packed: pack_unorm3(ambient),
            sky_ambient: ambient * 1.42,
            ground_ambient: ambient * 1.33,
        }
    }
}

/// 3 x 10-bit unorm (`x | y << 10 | z << 20`), inputs clamped to 0..=1.
fn pack_unorm3(v: Vec3) -> u32 {
    let q = |c: f32| (c.clamp(0.0, 1.0) * 1023.0).round() as u32;
    q(v.x) | q(v.y) << 10 | q(v.z) << 20
}

impl Renderer {
    pub(crate) fn v2_seed(&self) -> Seed {
        Seed { device: self.device.clone(), format: self.format, g_layout: self.g_layout.clone(), tex_layout: self.tex_layout.clone(),
            holo_layout: self.holo_layout.clone(), shadow_pipelines: self.shadow_pipelines.clone() }
    }

    /// Starts building [`V2`] (shader modules, pipelines, buffers) on a worker thread; idempotent.
    pub(crate) fn start_v2(&mut self) {
        if self.v2.is_none() && self.v2_build.is_none() {
            let seed = self.v2_seed();
            self.v2_build = Some(crate::variant::spawn_compile_worker("enhanced v2 build", move || V2::new(&seed)));
        }
    }

    /// Installs the finished build. Never blocks unless `block`. Returns whether `self.v2` is usable.
    pub(crate) fn finish_v2(&mut self, block: bool) -> bool {
        if self.v2.is_some() { return true; }
        let Some(build) = &self.v2_build else { return false };
        if !block && !build.is_finished() { return false; }
        match self.v2_build.take().expect("checked above").join() {
            Ok(v2) => { self.v2 = Some(v2); true }
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }

    /// Per frame: applies the v2 settings that were held back while [`V2`] was building (the previous path kept rendering).
    pub(crate) fn poll_v2(&mut self) {
        if let Some(settings) = self.v2_waiting {
            if self.finish_v2(false) { self.v2_waiting = None; self.request_enhanced_settings(settings); }
        }
    }

    /// v2 replaces the world pass only; GUI viewports, depth-clear layers and other native screens stay faithful.
    pub(crate) fn v2_eligible(&self) -> bool {
        self.screen.layers.is_empty() && self.screen.depth_clears.is_empty()
    }

    /// Per-pass GPU timing for the offline harness (allocates timestamp queries; off by default).
    pub fn set_v2_pass_timing(&mut self, enabled: bool) -> anyhow::Result<()> {
        self.start_v2();
        self.finish_v2(true);
        let timers = enabled.then(|| timers::PassTimers::new(&self.device, &self.queue)).flatten();
        anyhow::ensure!(timers.is_some() || !enabled, "v2 pass timing needs TIMESTAMP_QUERY");
        let v2 = self.v2.as_mut().expect("created above");
        v2.timers = timers;
        v2.live_timers = false;
        Ok(())
    }

    /// Ctrl+Shift+O overlay / slow-frame log: while the live profiler is on, time the v2 passes (rows named as the fixture's:
    /// shadows, gbuffer, hiz, ao, ..., post, `frame`) and feed the oldest completed ring slot into its rolling costs.
    /// Nothing is allocated or read back while the profiler is off.
    fn sync_live_v2_timers(&self, v2: &mut V2) {
        let Some(profiler) = &self.effect_profiler else {
            if v2.live_timers { v2.timers = None; v2.live_timers = false; }
            return;
        };
        if v2.timers.is_none() {
            v2.timers = timers::PassTimers::new(&self.device, &self.queue);
            v2.live_timers = v2.timers.is_some();
        }
        if !v2.live_timers { return; }
        // Lag 3 of the 4-slot ring is the slot about to be reused: its frame finished long ago, so the wait is ~free.
        if let Some(Ok(rows)) = v2.timers.as_ref().map(|timers| timers.read(&self.device, 3)) {
            if !rows.is_empty() { profiler.borrow_mut().record_external(&rows); }
        }
    }

    /// Per-pass GPU milliseconds of the last submitted v2 frame (blocks until it completes). Empty without timing.
    pub fn v2_pass_ms(&mut self, lag: usize) -> anyhow::Result<Vec<(&'static str, f64)>> {
        self.v2.as_ref().and_then(|v2| v2.timers.as_ref()).map_or(Ok(Vec::new()), |timers| timers.read(&self.device, lag))
    }

    pub(crate) fn render_v2(&mut self, resolve: &wgpu::TextureView, t: &Targets, cam: &Camera) -> wgpu::SubmissionIndex {
        let _compile_scope = crate::pipeline_creation::FrameGuard::enter();
        self.start_v2();
        self.finish_v2(true);
        let mut v2 = self.v2.take().expect("v2 built above");
        self.sync_live_v2_timers(&mut v2);
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
            let ao = ao::Ao::new(&self.device, &v2.ao, &v2.consts, &v2.rects, &pyramid, &targets);
            let resolve_bind = v2.resolve.bind(&self.device, &v2.consts, &targets, Some(ao.view()));
            let bloom = v2.bloom.bind(&self.device, &v2.consts, &targets);
            let taa = v2.taa_pass.bind(&self.device, &v2.consts, &v2.ctl, &v2.rects, &bloom.halo, &targets);
            let post_binds = [0, 1].map(|k| v2.post.bind(&self.device, &v2.consts, &v2.ctl, &taa.hist[1 - k], &targets.depth));
            let skyfix_bind = v2.skyfix.bind(&self.device, &targets.g1);
            v2.sized = Some(Sized { targets, resolve_bind, pyramid, ao, bloom, taa, post_binds, skyfix_bind });
            v2.taa_key = None;
            v2.region.clear();
            v2.taa_index = 0;
        }
        let sized = v2.sized.as_ref().expect("sized above");
        let look = Look::from_env(&env);
        // Moon A: the shadowed night key, lit by the authored moon pose; colour per DESIGN 4.2 (art tuning, not authored data).
        let moon = self.shadow_moon.and_then(|m| {
            let budget = Vec3::from(m.world_gamma_budget?);
            Some((Vec3::from(m.toward_light).normalize_or_zero(), Vec3::new(0.235, 0.39, 0.55) * 0.4 * (budget.max_element() / 0.2).clamp(0.0, 1.0)))
        });
        // Order-independent, non-cancelling hash of the actor items actually DRAWN this frame (post-cull actor layer, not every host
        // frame: projectiles/props that never reach a pixel list do not churn it); traced only. The temporal histories are invalidated per
        // REGION: items entering/leaving the drawn set drop the TAA/AO history inside their padded screen rects (region.rs, DESIGN section 8),
        // and the whole history only on a massive set change.
        let (admission, drawn_items) = admission::drawn_set_hash(self.drawn_actor_items());
        let previous_unjit = if v2.frame == 0 { view_proj_unjit } else { v2.prev_view_proj };
        let change = v2.region.update(self.drawn_actor_spheres(), previous_unjit, view_proj_unjit, size);
        // `AOMAC_V2_OFF=admission-region` (diagnostic, read once): any set change resets everything again, the pre-region behaviour, for before/after.
        let (admission_full, regions) = match &change {
            region::Change::Full => (true, &[][..]),
            region::Change::Rects(rects) if off("admission-region") && !rects.is_empty() => (true, &[][..]),
            region::Change::Rects(rects) => (false, &rects[..]),
        };
        self.queue.write_buffer(&v2.rects, 0, bytemuck::bytes_of(&region::RectUniform::new(regions)));
        // TAA history is discarded on frame 0 / resize, camera cuts and a massive drawn-set change; every actor pose of the motion pass
        // shares that decision.
        let key = taa::HistoryKey { eye: cam.pos, forward: cam.forward() };
        let taa_reset = if v2.taa { taa::reset_reason(v2.taa_key.as_ref(), &key, admission_full) } else { Some("taa-off") };
        let history_reset = taa_reset.is_some();
        v2.taa_key = Some(key);
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
            flags: [v2.frame, u32::from(!off("ao")), admission, look.native_ambient_packed],
        };
        self.queue.write_buffer(&v2.consts, 0, bytemuck::bytes_of(&consts));
        let post_ctl = post::Ctl { bloom: self.enhanced_settings.bloom_strength.clamp(0.0, 1.0), cas: if off("cas") { 0.0 } else { self.enhanced_settings.cas_strength.clamp(0.0, 1.0) },
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
        v2.skyfix.encode(&mut enc, &sized.skyfix_bind, &targets.depth);
        // [slot 1] actor admission: only this frame's actors (alpha > 0) are drawn into an id target depth-tested against the G-buffer
        // depth and reduced to `v2.admission.visibility()` (layout in admission.rs), read by the actor shadow layers and by lamps.
        v2.admission.encode(self, &mut enc, &targets.depth, size);
        // [slot 2] shadow maps: sun cascades + moon A, a cached static layer (world only) and a per-frame actor layer each.
        let light = |dir: Vec3| shadows::Light { dir, strength: crate::sun_shadows::horizon_fade(dir.y) };
        let shadow_view = shadows::View { eye: cam.pos, forward: cam.forward(), up: cam.up(), fovy: self.lens.vertical_fov(aspect), aspect,
            near: self.lens.near, far, fog_end: env.fog_end, time: self.time };
        v2.shadows.encode(self, &mut enc, &shadow_view, (look.sun_up > 0.5).then(|| light(look.sun)), moon.map(|(dir, _)| light(dir)), &v2.admission);
        // [slot 3] pyramid + AO (writes the AO texture the resolve reads, flags.y bit 0), [slot 4] light clusters + lamp cube atlas.
        sized.pyramid.encode(&v2.pyramid, &mut enc, timer.and_then(|t| t.compute("hiz")));
        let ao_reset = if off("ao") { None } else { sized.ao.encode(&v2.ao, &self.queue, &mut enc, timer, admission_full, jitter) };
        let lamps_bind = self.encode_lamps(&mut v2.lamps, &mut enc, timer, &v2.consts, cam, view_proj, [env.fog_start, env.fog_end, far]);
        v2.resolve.encode(&mut enc, &sized.resolve_bind, &[&v2.shadows.bind, &lamps_bind], size, timer.and_then(|t| t.compute("resolve")));
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
        v2.motion.encode(self, &mut enc, &targets.depth, &targets.g2, || timer.and_then(|t| t.render("motion")));
        if !off("bloom") { v2.bloom.encode(&mut enc, &sized.bloom, timer.and_then(|t| t.compute("bloom"))); }
        v2.taa_pass.encode(&mut enc, &sized.taa, v2.taa_index, size, timer.and_then(|t| t.compute("taa")));
        // `AOMAC_V2_OFF=post2` (harness) encodes post twice: the frame delta is post's marginal cost (its own span is not reliable on Metal).
        if off("post2") { v2.post.encode(&mut enc, &sized.post_binds[v2.taa_index], resolve, &targets.depth, None); }
        v2.post.encode(&mut enc, &sized.post_binds[v2.taa_index], resolve, &targets.depth, timer.and_then(|t| t.render("post")));
        v2.motion.finish(self, &mut enc);
        v2.taa_index ^= 1;
        if let Some(timers) = &v2.timers { timers.tail(&mut enc); timers.resolve(&mut enc); }

        self.drawn_visuals.extend(self.pending_visuals.get_mut().drain());
        self.stats = crate::FrameStats { instances: self.vis.len(), draw_calls: calls };
        v2.prev_view_proj = view_proj_unjit;
        if taa::trace_enabled() {
            taa::trace(&taa::TraceFrame { frame: v2.frame, time: self.time, eye: cam.pos, forward: cam.forward(), jitter, admission, drawn_items,
                regions: regions.len(), taa_reset, ao_reset });
        }
        v2.frame = v2.frame.wrapping_add(1);
        let submission = self.queue.submit([enc.finish()]);
        if let Some(timers) = &v2.timers { timers.submitted(&submission); }
        self.v2 = Some(v2);
        submission
    }
}

#[cfg(test)]
mod look_tests {
    use super::*;

    #[test]
    fn native_ambient_floor_round_trips_through_flags_w() {
        let p = pack_unorm3(Vec3::new(0.0, 0.5, 2.0));
        assert_eq!((p & 1023, p >> 10 & 1023, p >> 20 & 1023), (0, 512, 1023));
    }
}

#[cfg(test)]
mod switch_tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Selecting v2 at runtime must not build it on the render thread: the request returns at once, the previous path (and its
    /// settings) keep rendering until the worker lands, then the next frame swaps v2 in.
    #[test]
    fn runtime_selection_builds_off_the_render_thread() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut r = Renderer::new(&instance, None).expect("v2 switch regression requires a GPU");
        r.wait_for_pipeline_warmup();
        let v2 = crate::EnhancedSettings { enhanced_v2: true, ..Default::default() };
        let started = Instant::now();
        r.request_enhanced_settings(v2);
        let request = started.elapsed();
        eprintln!("v2 request returned in {request:?}");
        assert!(request < Duration::from_millis(100), "request blocked the caller for {request:?}");
        if r.v2.is_none() {
            assert!(!r.enhanced_settings.enhanced_v2, "settings were applied before v2 was ready");
            assert!(r.v2_waiting.is_some());
        }
        let output = r.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("v2 switch"), size: wgpu::Extent3d { width: 64, height: 64, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: r.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[],
        });
        let view = output.create_view(&Default::default());
        let targets = Targets::new(&r, 64, 64);
        let camera = Camera::look_at(Vec3::ZERO, Vec3::NEG_Z);
        let mut worst = Duration::ZERO;
        while !r.enhanced_settings.enhanced_v2 {
            let frame = Instant::now();
            r.render(&view, &targets, &camera);
            worst = worst.max(frame.elapsed());
            assert!(started.elapsed() < Duration::from_secs(120), "v2 never became ready");
            std::thread::sleep(Duration::from_millis(5));
        }
        eprintln!("v2 ready after {:?}, worst old-path frame {worst:?}", started.elapsed());
        assert!(r.v2.is_some() && r.v2_waiting.is_none());
        let frame = Instant::now();
        r.render(&view, &targets, &camera);
        eprintln!("first v2 frame {:?}", frame.elapsed());

        // A cancelled selection never applies later.
        r.request_enhanced_settings(crate::EnhancedSettings::default());
        assert!(r.v2_waiting.is_none());
    }
}
