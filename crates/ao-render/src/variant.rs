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
    pub shader: Option<wgpu::ShaderModule>,
    pub msaa: u32,
    pub sky_isolation: bool,
    pipeline_layout: wgpu::PipelineLayout,
    holo_pipeline_layout: wgpu::PipelineLayout,
    pub pipes: Pipelines,
    pub actor_pipes: Pipelines,
    pub ground_pipes: Pipelines,
    pub depth_clear_pipe: wgpu::RenderPipeline,
}

/// Owned handles let the frame borrow pipeline slots mutably while compiling a newly used slot.
pub(super) struct Compile {
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
    isolate_sky: bool,
    warm: (Vec<usize>, Vec<usize>, Vec<usize>),
    compiles: Arc<AtomicU64>,
    frame_compiles: Arc<AtomicU64>,
}

impl Request {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        device: &wgpu::Device, native: &wgpu::ShaderModule, format: wgpu::TextureFormat,
        layouts: (&wgpu::BindGroupLayout, &wgpu::BindGroupLayout, &wgpu::BindGroupLayout),
        key: Key, settings: EnhancedSettings, isolate_sky: bool,
        warm: (&Pipelines, &Pipelines, &Pipelines),
        compiles: &Arc<AtomicU64>, frame_compiles: &Arc<AtomicU64>,
    ) -> Self {
        Self {
            device: device.clone(), native: native.clone(), format,
            g_layout: layouts.0.clone(), tex_layout: layouts.1.clone(), holo_layout: layouts.2.clone(),
            key, settings, isolate_sky,
            warm: (warm.0.compiled_slots(), warm.1.compiled_slots(), warm.2.compiled_slots()),
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
        for index in &self.warm.0 { pipes.ensure(*index, &|spec: PipelineSpec| compile.pipeline(spec, self.isolate_sky && spec.sky)); }
        for index in &self.warm.1 { actor_pipes.ensure(*index, &|spec| compile.pipeline(spec, false)); }
        for index in &self.warm.2 { ground_pipes.ensure(*index, &|spec| compile.pipeline(spec, false)); }
        self.compiles.fetch_add(1, Ordering::Relaxed);
        if frame_thread { self.frame_compiles.fetch_add(1, Ordering::Relaxed); }
        let depth_clear_pipe = super::screen::depth_clear_pipeline(&self.device, self.format, self.key.msaa, self.settings);
        Pipes { shader, msaa: self.key.msaa, sky_isolation: self.isolate_sky,
            pipeline_layout, holo_pipeline_layout, pipes, actor_pipes, ground_pipes, depth_clear_pipe }
    }

    pub(super) fn spawn(self, key: Key, settings: EnhancedSettings) -> Pending {
        let worker = std::thread::Builder::new().name("enhanced world variant".into())
            .spawn(move || self.build(false))
            .expect("spawn the enhanced world variant thread");
        Pending { key, settings, worker: Some(worker) }
    }
}

impl Pipes {
    pub(super) fn compile(&self, device: &wgpu::Device, native: &wgpu::ShaderModule,
        format: wgpu::TextureFormat, settings: EnhancedSettings,
        compiles: Arc<AtomicU64>, frame_compiles: Arc<AtomicU64>) -> Compile {
        Compile { device: device.clone(), shader: self.shader.clone().unwrap_or_else(|| native.clone()),
            native: native.clone(), pipeline_layout: self.pipeline_layout.clone(),
            holo_pipeline_layout: self.holo_pipeline_layout.clone(), format, settings, msaa: self.msaa,
            compiles, frame: Some(frame_compiles) }
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
