//! Cached world-shading combinations. Cold live switches compile off the frame thread.
use std::sync::{Arc, atomic::{AtomicU64, Ordering}};

use super::{EnhancedSettings, Pipelines, PipelineSpec, build_pipelines, compile_pipeline};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Key {
    shadows: bool,
    ao: bool,
    bloom: bool,
    sun_mask: bool,
    pub msaa: u32,
}

impl Key {
    pub(super) fn of(settings: EnhancedSettings, msaa: u32) -> Self {
        Self { shadows: settings.sun_shadows, ao: settings.ambient_occlusion,
            bloom: settings.emissive_bloom, sun_mask: settings.needs_sun_mask(), msaa }
    }
    fn enhanced(self) -> bool { self.shadows || self.ao || self.bloom || self.sun_mask }
}

pub(super) struct Pipes {
    #[cfg(test)]
    pub shader: Option<wgpu::ShaderModule>,
    pub msaa: u32,
    pub native_sky_pipes: Option<Pipelines>,
    pub pipes: Pipelines,
    pub actor_pipes: Pipelines,
    pub ground_pipes: Pipelines,
    pub depth_clear_pipe: wgpu::RenderPipeline,
    pub grass: super::grass::Compiled,
}

/// A variant is completely compiled at startup or on the existing switch worker.
struct Compile {
    device: wgpu::Device,
    shader: wgpu::ShaderModule,
    native: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    holo_pipeline_layout: wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    settings: EnhancedSettings,
    msaa: u32,
    compiles: Arc<AtomicU64>,
    frame: Option<Arc<AtomicU64>>,
}

impl Compile {
    pub(super) fn pipeline(&self, spec: PipelineSpec, faithful_sky: bool) -> wgpu::RenderPipeline {
        self.compiles.fetch_add(1, Ordering::Relaxed);
        if let Some(frame) = &self.frame { frame.fetch_add(1, Ordering::Relaxed); }
        let (shader, settings) = if faithful_sky { (&self.native, EnhancedSettings::default()) } else { (&self.shader, self.settings) };
        compile_pipeline(&self.device, shader, self.format, (&self.pipeline_layout, &self.holo_pipeline_layout), settings, self.msaa, spec)
    }
}

pub(super) struct Request {
    device: wgpu::Device,
    native: wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    g_layout: wgpu::BindGroupLayout,
    tex_layout: wgpu::BindGroupLayout,
    holo_layout: wgpu::BindGroupLayout,
    key: Key,
    settings: EnhancedSettings,
    compiles: Arc<AtomicU64>,
    frame_compiles: Arc<AtomicU64>,
}

impl Request {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        device: &wgpu::Device, native: &wgpu::ShaderModule, format: wgpu::TextureFormat,
        layouts: (&wgpu::BindGroupLayout, &wgpu::BindGroupLayout, &wgpu::BindGroupLayout),
        key: Key, settings: EnhancedSettings,
        compiles: &Arc<AtomicU64>, frame_compiles: &Arc<AtomicU64>,
    ) -> Self {
        Self {
            device: device.clone(), native: native.clone(), format,
            g_layout: layouts.0.clone(), tex_layout: layouts.1.clone(), holo_layout: layouts.2.clone(),
            key, settings,
            compiles: compiles.clone(), frame_compiles: frame_compiles.clone(),
        }
    }

    pub(super) fn build(&self, frame_thread: bool) -> Pipes {
        let shader = self.key.enhanced().then(|| {
            let source = super::enhanced_shader::source(
                self.settings.sun_shadows, self.settings.ambient_occlusion,
                self.settings.emissive_bloom, self.settings.needs_sun_mask());
            self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Enhanced world"), source: wgpu::ShaderSource::Wgsl(source.into()),
            })
        });
        let shadow_layout = self.key.shadows.then(|| super::sun_shadows::receiver_layout(&self.device));
        let mut ordinary = vec![Some(&self.g_layout), Some(&self.tex_layout)];
        let mut holo = vec![Some(&self.g_layout), Some(&self.tex_layout), Some(&self.holo_layout)];
        if let Some(shadow_layout) = shadow_layout.as_ref() {
            ordinary.extend([None, Some(shadow_layout)]);
            holo.push(Some(shadow_layout));
        }
        let pipeline_layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("world optional shadow receiver"), bind_group_layouts: &ordinary, immediate_size: 0,
        });
        let holo_pipeline_layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("holo optional shadow receiver"), bind_group_layouts: &holo, immediate_size: 0,
        });
        let compile = Compile {
            device: self.device.clone(), shader: shader.clone().unwrap_or_else(|| self.native.clone()),
            native: self.native.clone(), pipeline_layout: pipeline_layout.clone(),
            holo_pipeline_layout: holo_pipeline_layout.clone(), format: self.format,
            settings: self.settings, msaa: self.key.msaa, compiles: self.compiles.clone(),
            frame: frame_thread.then(|| self.frame_compiles.clone()),
        };
        let (mut pipes, mut actor_pipes, mut ground_pipes) = build_pipelines();
        pipes.prepare_all(&|spec| compile.pipeline(spec, false));
        actor_pipes.prepare_all(&|spec| compile.pipeline(spec, false));
        ground_pipes.prepare_all(&|spec| compile.pipeline(spec, false));
        let native_sky_pipes = self.key.enhanced().then(|| {
            let (mut sky, _, _) = build_pipelines();
            for index in 0..sky.len() {
                if sky.spec(index).sky { sky.ensure(index, &|spec| compile.pipeline(spec, true)); }
            }
            sky
        });
        self.compiles.fetch_add(1, Ordering::Relaxed);
        if frame_thread { self.frame_compiles.fetch_add(1, Ordering::Relaxed); }
        let depth_clear_pipe = super::screen::depth_clear_pipeline(&self.device, self.format, self.key.msaa, self.settings);
        let grass = super::grass::Compiled::new(
            &self.device, &self.g_layout, &self.tex_layout, &self.holo_layout, self.key.msaa,
            (self.settings.ambient_occlusion, self.settings.emissive_bloom, self.settings.needs_sun_mask()),
            self.settings.sun_shadows, self.format);
        Pipes { #[cfg(test)] shader, msaa: self.key.msaa, native_sky_pipes,
            pipes, actor_pipes, ground_pipes, depth_clear_pipe, grass }
    }

    pub(super) fn spawn(self, key: Key, settings: EnhancedSettings) -> Pending {
        let worker = std::thread::Builder::new().name("enhanced world variant".into())
            .spawn(move || self.build(false))
            .expect("spawn the enhanced world variant thread");
        Pending { key, settings, worker: Some(worker) }
    }
}


pub(super) struct Pending {
    pub key: Key,
    pub settings: EnhancedSettings,
    worker: Option<std::thread::JoinHandle<Pipes>>,
}

impl Pending {
    pub(super) fn poll(&mut self) -> Option<Pipes> {
        self.worker.as_ref().expect("pending worker").is_finished().then(|| self.wait())
    }
    pub(super) fn wait(&mut self) -> Pipes {
        match self.worker.take().expect("pending worker").join() {
            Ok(world) => world,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Renderer;

    #[test]
    fn standalone_live_msaa_request_compiles_off_frame_thread_and_reuses_cache() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut r = Renderer::new(&instance, None).expect("MSAA state-machine regression requires GPU");
        let samples = r.msaa_supported.iter().copied().find(|&samples| samples > 1)
            .expect("MSAA state-machine regression requires a multisample-capable GPU");
        let original = r.msaa_samples();
        assert_ne!(samples, original);
        let draw = |r: &mut Renderer| {
            let targets = crate::Targets::new(r, 64, 64);
            let output = r.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("standalone live MSAA regression"),
                size: wgpu::Extent3d { width: 64, height: 64, depth_or_array_layers: 1 },
                mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
                format: r.format, usage: wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[],
            });
            r.render(&output.create_view(&Default::default()), &targets,
                &crate::Camera::look_at(crate::Vec3::new(0.0, 1.0, 5.0), crate::Vec3::ZERO));
            r.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        };
        draw(&mut r);
        let world_frame_compiles = r.world_frame_thread_compiles();
        let total_frame_compiles = r.frame_thread_compiles();
        let _frame = crate::pipeline_creation::FrameGuard::enter();
        r.set_world_options(Some(samples), None, None);
        assert!(r.pending_world.is_some(), "a cold standalone MSAA option must spawn a worker");
        assert_eq!(r.msaa_samples(), original, "cold request replaced active samples before installation");
        assert_eq!(r.world_frame_thread_compiles(), world_frame_compiles, "cold MSAA request compiled world pipelines in frame");
        assert_eq!(r.frame_thread_compiles(), total_frame_compiles, "cold MSAA request created render or compute pipelines in frame");
        draw(&mut r);
        assert_eq!(r.msaa_samples(), original, "render installed a pending variant mid-frame");
        let waiting = std::time::Instant::now();
        while !r.pending_world.as_ref().unwrap().worker.as_ref().unwrap().is_finished() {
            assert_eq!(r.msaa_samples(), original, "active samples changed while worker was compiling");
            assert!(waiting.elapsed() < std::time::Duration::from_secs(600), "MSAA worker did not finish");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(r.msaa_samples(), original, "completed worker must wait for explicit frame-boundary settlement");
        r.settle_enhanced_world();
        assert!(r.pending_world.is_none());
        assert_eq!(r.msaa_samples(), samples);
        draw(&mut r);
        assert_eq!(r.world_frame_thread_compiles(), world_frame_compiles, "MSAA install/first draw compiled world pipelines in frame");
        assert_eq!(r.frame_thread_compiles(), total_frame_compiles, "MSAA install/first draw created render or compute pipelines in frame");
        let warmed_compiles = r.world_pipeline_compiles();
        for expected in [original, samples] {
            r.set_world_options(Some(expected), None, None);
            assert!(r.pending_world.is_none(), "cached MSAA return must install immediately");
            assert_eq!(r.msaa_samples(), expected);
            draw(&mut r);
            assert_eq!(r.world_pipeline_compiles(), warmed_compiles, "cached MSAA roundtrip compiled");
            assert_eq!(r.world_frame_thread_compiles(), world_frame_compiles, "cached MSAA roundtrip compiled world pipelines in frame");
            assert_eq!(r.frame_thread_compiles(), total_frame_compiles, "cached MSAA roundtrip created render or compute pipelines in frame");
        }
    }

    #[test]
    fn live_variant_requests_preserve_workers_and_supersede_stale_results() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut r = Renderer::new(&instance, None).expect("variant state-machine regression requires GPU");
        let faithful = EnhancedSettings::default();
        let ao = EnhancedSettings { ambient_occlusion: true, ..faithful };
        let bloom = EnhancedSettings { emissive_bloom: true, ..faithful };

        r.request_enhanced_settings(ao);
        let worker = r.pending_world.as_ref().unwrap().worker.as_ref().unwrap().thread().id();
        let adjusted = EnhancedSettings { ao_strength: 0.5, ..ao };
        for _ in 0..3 {
            r.set_world_options(Some(1), None, Some(adjusted));
            let pending = r.pending_world.as_ref().expect("repeated request must stay pending");
            assert_eq!(pending.worker.as_ref().unwrap().thread().id(), worker, "same request restarted compilation");
            assert_eq!(pending.settings, adjusted, "knobs must travel with the existing worker");
            assert_eq!(r.enhanced_settings(), faithful, "cold requests must preserve active settings");
        }
        // Keep the real worker so this test can wait for its stale result deterministically.
        let stale = r.pending_world.as_mut().unwrap().worker.take().unwrap();
        r.request_enhanced_settings(faithful);
        assert!(r.pending_world.is_none(), "switching back to active must cancel the pending install");
        stale.join().expect("cancelled worker must finish");
        r.settle_enhanced_world();
        assert_eq!(r.enhanced_settings(), faithful, "cancelled result installed");

        r.set_enhanced_settings(ao);
        r.set_enhanced_settings(faithful);
        let frame_compiles = r.world_frame_thread_compiles();
        r.request_enhanced_settings(bloom);
        let stale = r.pending_world.as_mut().unwrap().worker.take().unwrap();
        r.request_enhanced_settings(ao);
        assert!(r.pending_world.is_none(), "cached install must supersede the pending worker");
        assert_eq!(r.enhanced_settings(), ao);
        stale.join().expect("superseded worker must finish");
        let compiles = r.world_pipeline_compiles();
        r.settle_enhanced_world();
        assert_eq!(r.enhanced_settings(), ao, "superseded result replaced cached settings");
        r.request_enhanced_settings(faithful);
        r.request_enhanced_settings(ao);
        assert_eq!(r.world_pipeline_compiles(), compiles, "warm roundtrip compiled");
        assert_eq!(r.world_frame_thread_compiles(), frame_compiles, "live requests compiled on frame thread");
    }
}
