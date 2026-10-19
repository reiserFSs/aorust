//! Bloom (slot 5a): 5-level dual filter from half resolution, seeded only by G3 authored emission (DESIGN 1.9, glow rule 10).
//!
//! One compute pass (timer `bloom`): seed (G3 x native transmittance, soft knee) -> 4 downsamples -> 4 upsamples -> composite
//! (HDR + halo) into the full-resolution TAA input. Added back in HDR before TAA at `EnhancedSettings::bloom_strength`
//! (default 0.6): the chain is an energy-preserving average of the five levels, so that share of the emitted radiance is
//! redistributed as halo.
use super::{gbuffer::{Targets, HDR}, post::{self, compute_pipeline}};

const LEVELS: u32 = 5;
const C: wgpu::ShaderStages = wgpu::ShaderStages::COMPUTE;

pub(super) struct Bloom {
    seed: wgpu::ComputePipeline,
    down: wgpu::ComputePipeline,
    up: wgpu::ComputePipeline,
    composite: wgpu::ComputePipeline,
    layouts: [wgpu::BindGroupLayout; 4],
    sampler: wgpu::Sampler,
}

/// Size-dependent resources: the two mip chains' bind groups and the full-resolution output.
pub(super) struct BloomTargets {
    /// HDR + bloom, the TAA input.
    pub comp: wgpu::TextureView,
    /// (pipeline kind, bind group, dispatch size) in execution order.
    steps: Vec<(Kind, wgpu::BindGroup, (u32, u32))>,
}

#[derive(Clone, Copy)]
enum Kind { Seed, Down, Up, Composite }

fn chain(device: &wgpu::Device, label: &str, size: (u32, u32), levels: u32) -> (wgpu::Texture, Vec<wgpu::TextureView>) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label), size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
        mip_level_count: levels, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: HDR,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
    });
    let views = (0..levels).map(|mip| texture.create_view(&wgpu::TextureViewDescriptor {
        base_mip_level: mip, mip_level_count: Some(1), ..Default::default() })).collect();
    (texture, views)
}

fn level_size(half: (u32, u32), mip: u32) -> (u32, u32) { ((half.0 >> mip).max(1), (half.1 >> mip).max(1)) }

impl Bloom {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let layout = |label, entries: &[wgpu::BindGroupLayoutEntry]| device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some(label), entries });
        let layouts = [
            layout("v2 bloom seed", &[post::uniform_entry(0, C), post::tex_entry(1, C, false), post::depth_entry(2, C), post::storage_entry(6)]),
            layout("v2 bloom down", &[post::tex_entry(3, C, true), post::sampler_entry(5), post::storage_entry(6)]),
            layout("v2 bloom up", &[post::tex_entry(3, C, true), post::tex_entry(4, C, false), post::sampler_entry(5), post::storage_entry(6)]),
            layout("v2 bloom composite", &[post::uniform_entry(0, C), post::depth_entry(2, C), post::tex_entry(3, C, true), post::sampler_entry(5),
                post::storage_entry(6), post::tex_entry(7, C, false), post::uniform_entry(8, C)]),
        ];
        let module = post::module(device, "v2 bloom", crate::shader_source::load("v2/bloom.wgsl", include_str!("bloom.wgsl")));
        Self {
            seed: compute_pipeline(device, "v2 bloom seed", &module, "seed", &layouts[0]),
            down: compute_pipeline(device, "v2 bloom down", &module, "down", &layouts[1]),
            up: compute_pipeline(device, "v2 bloom up", &module, "up", &layouts[2]),
            composite: compute_pipeline(device, "v2 bloom composite", &module, "composite", &layouts[3]),
            layouts,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor { mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear,
                address_mode_u: wgpu::AddressMode::ClampToEdge, address_mode_v: wgpu::AddressMode::ClampToEdge, ..Default::default() }),
        }
    }

    pub(super) fn bind(&self, device: &wgpu::Device, consts: &wgpu::Buffer, ctl: &wgpu::Buffer, t: &Targets) -> BloomTargets {
        let size = t.size;
        let half = (size.0.div_ceil(2), size.1.div_ceil(2));
        let (_down_tex, down) = chain(device, "v2 bloom down", half, LEVELS);
        let (_up_tex, up) = chain(device, "v2 bloom up", half, LEVELS - 1);
        let comp = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("v2 bloom composite"), size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: HDR,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
        }).create_view(&Default::default());
        let group = |layout: &wgpu::BindGroupLayout, entries: &[wgpu::BindGroupEntry]| device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("v2 bloom"), layout, entries });
        let sampler = |binding| wgpu::BindGroupEntry { binding, resource: wgpu::BindingResource::Sampler(&self.sampler) };
        let uniform = post::buffer_bind;
        let mut steps = vec![(Kind::Seed, group(&self.layouts[0], &[uniform(0, consts), post::view_bind(1, &t.g3), post::view_bind(2, &t.depth), post::view_bind(6, &down[0])]), half)];
        for mip in 1..LEVELS as usize {
            steps.push((Kind::Down, group(&self.layouts[1], &[post::view_bind(3, &down[mip - 1]), sampler(5), post::view_bind(6, &down[mip])]), level_size(half, mip as u32)));
        }
        for mip in (0..LEVELS as usize - 1).rev() {
            let lower = if mip + 1 == LEVELS as usize - 1 { &down[mip + 1] } else { &up[mip + 1] };
            steps.push((Kind::Up, group(&self.layouts[2], &[post::view_bind(3, lower), post::view_bind(4, &down[mip]), sampler(5), post::view_bind(6, &up[mip])]), level_size(half, mip as u32)));
        }
        steps.push((Kind::Composite, group(&self.layouts[3], &[uniform(0, consts), post::view_bind(2, &t.depth), post::view_bind(3, &up[0]), sampler(5),
            post::view_bind(6, &comp), post::view_bind(7, &t.hdr), uniform(8, ctl)]), size));
        BloomTargets { comp, steps }
    }

    pub(super) fn encode(&self, enc: &mut wgpu::CommandEncoder, targets: &BloomTargets, timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>) {
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("v2 bloom"), timestamp_writes });
        for (kind, bind, size) in &targets.steps {
            pass.set_pipeline(match kind { Kind::Seed => &self.seed, Kind::Down => &self.down, Kind::Up => &self.up, Kind::Composite => &self.composite });
            pass.set_bind_group(0, bind, &[]);
            pass.dispatch_workgroups(size.0.div_ceil(8), size.1.div_ceil(8), 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_has_five_levels_down_to_one_thirty_second() {
        let half = (960, 600);
        let sizes: Vec<_> = (0..LEVELS).map(|m| level_size(half, m)).collect();
        assert_eq!(sizes, [(960, 600), (480, 300), (240, 150), (120, 75), (60, 37)]);
        assert_eq!(level_size((1, 1), 4), (1, 1));
    }

    /// Glow rule (DESIGN section 10): seeds are authored emission x fog transmittance behind a soft knee on the brightest channel;
    /// never resolve highlights or a self-lit flag.
    #[test]
    fn seed_reads_only_emission_with_knee_and_fog() {
        let wgsl = include_str!("bloom.wgsl");
        assert!(wgsl.contains("textureLoad(g3t, q, 0)") && wgsl.contains("smoothstep(KNEE_LO, KNEE_HI, peak)") && wgsl.contains("t_native_depth("));
        assert!(!wgsl.contains("g0t") && !wgsl.contains("MF_"), "the seed must not look at material flags");
    }
}
