//! Stable native pipeline slots, compiled only when a submitted draw needs them.
use super::Blend;
use std::ops::Index;

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
#[derive(Clone, Copy)]
enum Source { Compile(Spec), Alias(usize) }
pub(super) struct Slot { source: Source, pipeline: Option<wgpu::RenderPipeline> }
impl Slot {
    pub fn new(spec: Spec) -> Self { Self { source: Source::Compile(spec), pipeline: None } }
    pub fn alias(index: usize) -> Self { Self { source: Source::Alias(index), pipeline: None } }
}
pub(super) struct Pipelines(Vec<Slot>);
impl From<Vec<Slot>> for Pipelines {
    fn from(slots: Vec<Slot>) -> Self { Self(slots) }
}
impl Pipelines {
    pub fn spec(&self, index: usize) -> Spec {
        match self.0[index].source {
            Source::Compile(spec) => spec,
            Source::Alias(source) => self.spec(source),
        }
    }
    pub fn ensure<F: Fn(Spec) -> wgpu::RenderPipeline>(&mut self, index: usize, compile: &F) {
        if self.0[index].pipeline.is_some() { return }
        let pipeline = match self.0[index].source {
            Source::Compile(spec) => compile(spec),
            Source::Alias(source) => { self.ensure(source, compile); self.0[source].pipeline.as_ref().unwrap().clone() },
        };
        self.0[index].pipeline = Some(pipeline);
    }
    pub fn invalidate(&mut self) {
        for slot in &mut self.0 { slot.pipeline = None; }
    }
}
impl Index<usize> for Pipelines {
    type Output = wgpu::RenderPipeline;
    fn index(&self, index: usize) -> &Self::Output {
        self.0[index].pipeline.as_ref().expect("native pipeline must be prepared before opening the render pass")
    }
}
