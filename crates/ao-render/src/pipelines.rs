//! Stable native pipeline slots, prepared before rendering begins.
use super::{Blend, pipeline_creation::RenderPipeline};
use std::{collections::HashMap, ops::Index};

#[derive(Clone, Copy)]
pub(super) struct Spec {
    pub blend: Blend,
    pub two_sided: bool,
    pub sky: bool,
    pub env: bool,
    pub fade: bool,
    pub sprite: bool,
    pub actor: bool,
    pub flip_depth: bool,
    pub effect: u32,
    pub compare: Option<wgpu::CompareFunction>,
}

/// All spec-dependent descriptor fields; device, shaders, targets and MSAA are
/// fixed for each preparation call. Keep compilation and deduplication on this
/// same selection path, including vertex entry points and instance layouts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct Descriptor {
    pub fs: &'static str,
    pub vs: &'static str,
    pub state: Option<wgpu::BlendState>,
    pub depth_write: bool,
    pub compare: wgpu::CompareFunction,
    pub actor: bool,
    pub holo: bool,
    pub points: bool,
    pub cull: bool,
    pub bias: bool,
}

impl Spec {
    pub fn descriptor(self) -> Descriptor {
        let Self { blend, two_sided, sky, env, fade, sprite, actor, flip_depth, effect, compare } = self;
        let add = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::SrcAlpha, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add };
        let one_one = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add };
        let (fs, state, depth_write) = match blend {
            _ if env => ("fs_env", Some(wgpu::BlendState { color: one_one, alpha: one_one }), false),
            _ if sprite => ("fs_sprite", Some(wgpu::BlendState::ALPHA_BLENDING), true),
            Blend::Opaque if fade => ("fs_fade_opaque", Some(wgpu::BlendState::ALPHA_BLENDING), true),
            Blend::AlphaTest if fade => ("fs_fade_test", Some(wgpu::BlendState::ALPHA_BLENDING), true),
            Blend::Opaque => ("fs_opaque", None, true),
            Blend::AlphaTest => ("fs_test", None, true),
            Blend::AlphaBlend => ("fs_blend", Some(wgpu::BlendState::ALPHA_BLENDING), false),
            Blend::Additive => ("fs_add", Some(wgpu::BlendState { color: add, alpha: wgpu::BlendComponent::OVER }), false),
            Blend::ZeroSourceColor | Blend::DestinationColorSourceColor | Blend::PremultipliedAlpha | Blend::SourceColorOne => ("fs_blend", Some(super::native_blend_state(blend)), false),
            Blend::AdditiveOne => ("fs_add_one", Some(wgpu::BlendState { color: one_one, alpha: one_one }), false),
        };
        let fs = if sky {
            match blend {
                Blend::Opaque => "fs_sky_opaque",
                Blend::AlphaTest => "fs_sky_test",
                Blend::Additive => "fs_sky_add",
                Blend::AdditiveOne => "fs_sky_add_one",
                _ => "fs_sky_blend",
            }
        } else { fs };
        let depth_write = if sky { flip_depth } else { depth_write ^ flip_depth };
        let (fs, state, depth_write) = match effect {
            1 => ("fs_holo", Some(wgpu::BlendState { color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::Src, operation: wgpu::BlendOperation::Add }, alpha: wgpu::BlendComponent::OVER }), false),
            // DS1006c005's RTriMesh mode 2 forces depth writes.
            2 => (fs, state, true),
            4..=6 => ("fs_mesh_add", Some(wgpu::BlendState { color: add, alpha: add }), false),
            8 => ("fs_ground_fused", None, true),
            9 => ("fs_ground_base", None, true),
            10 => ("fs_ground_shadow", Some(wgpu::BlendState { color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::Src, operation: wgpu::BlendOperation::Add }, alpha: wgpu::BlendComponent::REPLACE }), false),
            11 => ("fs_ground_simple", None, true),
            12 | 13 => {
                let component = wgpu::BlendComponent {
                    src_factor: if effect == 12 { wgpu::BlendFactor::SrcAlpha } else { wgpu::BlendFactor::OneMinusSrcAlpha },
                    dst_factor: if effect == 12 { wgpu::BlendFactor::OneMinusSrcAlpha } else { wgpu::BlendFactor::SrcAlpha },
                    operation: wgpu::BlendOperation::Add,
                };
                ("fs_blend", Some(wgpu::BlendState { color: component, alpha: component }), true)
            },
            14 => ("fs_sky_add", Some(wgpu::BlendState { color: one_one, alpha: one_one }), false),
            15..=17 => {
                let component = wgpu::BlendComponent {
                    src_factor: if effect == 16 { wgpu::BlendFactor::OneMinusSrcAlpha } else { wgpu::BlendFactor::SrcAlpha },
                    dst_factor: if effect == 16 { wgpu::BlendFactor::SrcAlpha } else { wgpu::BlendFactor::OneMinusSrcAlpha },
                    operation: wgpu::BlendOperation::Add,
                };
                (if effect == 17 { "fs_lava_crust" } else { "fs_lava" }, Some(wgpu::BlendState { color: component, alpha: component }), true)
            },
            18 => ("fs_foliage_blend_core", Some(wgpu::BlendState::ALPHA_BLENDING), true),
            19 => ("fs_foliage_blend_soft", Some(wgpu::BlendState::ALPHA_BLENDING), false),
            20 => ("fs_foliage_sprite_core", Some(wgpu::BlendState::ALPHA_BLENDING), true),
            21 => ("fs_foliage_sprite_soft", Some(wgpu::BlendState::ALPHA_BLENDING), false),
            // DS1003d13a/1003b9c9 blob +0x5d8: SRCALPHA/INVSRCALPHA, default Z write, LESSEQUAL.
            22 | 23 => ("fs_refraction", Some(wgpu::BlendState::ALPHA_BLENDING), true),
            _ => (fs, state, depth_write),
        };
        Descriptor {
            fs, state, depth_write, actor,
            vs: if effect == 14 { "vs_sky_points" } else if effect == 22 { "vs_refraction" } else if effect == 23 { "vs_refraction_shallow" } else if (15..=17).contains(&effect) { "vs_lava" } else if (8..=11).contains(&effect) { "vs_ground" } else if matches!(effect, 12 | 13) { "vs_liquid" } else if actor && matches!(effect, 1 | 4..=6) { "vs_actor" } else if sky { "vs_sky" } else if env { "vs_env" } else if actor { "vs_actor" } else { "vs" },
            holo: effect == 1 || (8..=11).contains(&effect),
            points: effect == 14,
            cull: !two_sided && !matches!(effect, 1 | 4..=6),
            // Randy 10041f98 and DS 1006c61d retain LESSEQUAL.
            compare: if effect == 2 { wgpu::CompareFunction::LessEqual } else { compare.unwrap_or(if sky { wgpu::CompareFunction::Always } else if env || matches!(effect, 0..=7 | 10 | 12 | 13 | 15..=23) { wgpu::CompareFunction::LessEqual } else { wgpu::CompareFunction::Less }) },
            bias: effect == 7,
        }
    }
}
#[derive(Clone, Copy)]
enum Source { Compile(Spec), Alias(usize) }
pub(super) struct Slot { source: Source, pipeline: Option<RenderPipeline> }
impl Slot {
    pub fn new(spec: Spec) -> Self { Self { source: Source::Compile(spec), pipeline: None } }
    pub fn alias(index: usize) -> Self { Self { source: Source::Alias(index), pipeline: None } }
}
pub(super) struct Pipelines(Vec<Slot>);
impl From<Vec<Slot>> for Pipelines {
    fn from(slots: Vec<Slot>) -> Self { Self(slots) }
}
impl Pipelines {
    pub fn len(&self) -> usize { self.0.len() }
    pub fn push(&mut self, slot: Slot) { self.0.push(slot); }
    pub fn prepare_all<F: Fn(Spec) -> RenderPipeline>(&mut self, compile: &F) {
        let mut prepared: HashMap<Descriptor, usize> = HashMap::new();
        for index in 0..self.len() {
            if let Source::Compile(spec) = self.0[index].source {
                let key = spec.descriptor();
                if let Some(&source) = prepared.get(&key) {
                    self.0[index].pipeline = self.0[source].pipeline.clone();
                } else {
                    self.ensure(index, compile);
                    prepared.insert(key, index);
                }
            } else {
                self.ensure(index, compile);
            }
        }
    }
    pub fn spec(&self, index: usize) -> Spec {
        match self.0[index].source {
            Source::Compile(spec) => spec,
            Source::Alias(source) => self.spec(source),
        }
    }
    pub fn ensure<F: Fn(Spec) -> RenderPipeline>(&mut self, index: usize, compile: &F) {
        if self.0[index].pipeline.is_some() { return }
        let pipeline = match self.0[index].source {
            Source::Compile(spec) => compile(spec),
            Source::Alias(source) => { self.ensure(source, compile); self.0[source].pipeline.as_ref().unwrap().clone() },
        };
        self.0[index].pipeline = Some(pipeline);
    }
    pub fn is_ready(&self) -> bool {
        self.0.iter().filter_map(|slot| slot.pipeline.as_ref()).all(RenderPipeline::is_ready)
    }
    pub fn wait(&self) {
        for pipeline in self.0.iter().filter_map(|slot| slot.pipeline.as_ref()) { pipeline.get(); }
    }
    pub fn warm(&self) {
        for pipeline in self.0.iter().filter_map(|slot| slot.pipeline.as_ref()) { pipeline.warm(); }
    }
}
impl Index<usize> for Pipelines {
    type Output = wgpu::RenderPipeline;
    fn index(&self, index: usize) -> &Self::Output {
        self.0[index].pipeline.as_ref().expect("native pipeline must be prepared before opening the render pass").get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn foliage_core_keeps_blended_coverage_and_soft_edges_do_not_write_depth() {
        let base = Spec { blend: Blend::AlphaBlend, two_sided: true, sky: false, env: false,
            fade: false, sprite: false, actor: false, flip_depth: false, effect: 18, compare: None };
        for effect in 18..=21 {
            let descriptor = Spec { effect, ..base }.descriptor();
            assert_eq!(descriptor.state, Some(wgpu::BlendState::ALPHA_BLENDING));
            assert_eq!(descriptor.depth_write, effect % 2 == 0);
        }
    }

    #[test]
    fn descriptor_duplicates_reuse_keys_without_merging_distinct_state() {
        let base = Spec { blend: Blend::Opaque, two_sided: false, sky: false, env: false,
            fade: false, sprite: false, actor: true, flip_depth: false, effect: 0, compare: None };
        assert_eq!(base.descriptor(), Spec { compare: Some(wgpu::CompareFunction::LessEqual), ..base }.descriptor());
        assert_eq!(base.descriptor(), Spec { effect: 2, ..base }.descriptor());
        assert_ne!(base.descriptor(), Spec { effect: 7, ..base }.descriptor());
        assert_ne!(base.descriptor(), Spec { actor: false, ..base }.descriptor());
        assert_ne!(base.descriptor(), Spec { two_sided: true, ..base }.descriptor());
        assert_ne!(base.descriptor(), Spec { flip_depth: true, ..base }.descriptor());
        assert_ne!(base.descriptor(), Spec { compare: Some(wgpu::CompareFunction::Always), ..base }.descriptor());

        let additive = Spec { effect: 4, ..base };
        let key = additive.descriptor();
        // Effects live in actor instance data; there are currently no vertex
        // specialization constants. These descriptors really are identical.
        for effect in 4..=6 {
            assert_eq!(key, Spec { effect, blend: Blend::SourceColorOne, two_sided: true,
                fade: true, flip_depth: true, ..base }.descriptor());
        }
        assert_ne!(key, Spec { actor: false, ..additive }.descriptor());
        assert_ne!(key, Spec { sky: true, ..additive }.descriptor());
        assert_ne!(key, Spec { effect: 1, ..base }.descriptor());
        assert_ne!(Spec { effect: 12, ..base }.descriptor(), Spec { effect: 13, ..base }.descriptor());

        let (_, actors, _) = super::super::build_pipelines();
        let keys: HashSet<_> = (0..actors.len()).filter_map(|index| {
            let spec = actors.spec(index);
            (4..=6).contains(&spec.effect).then(|| spec.descriptor())
        }).collect();
        // All three complete effect rows reduce to the three depth comparisons,
        // not hundreds of blend/depth-write/cull variations.
        assert_eq!(keys.len(), 3);
    }
}
