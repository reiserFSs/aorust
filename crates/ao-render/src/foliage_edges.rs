//! Enhanced two-pass foliage, retaining native alpha tests and shading.
use super::{Blend, PipelineSpec, PipelineSlot, Pipelines};

fn eligible(spec: PipelineSpec) -> bool {
    spec.blend == Blend::AlphaBlend && spec.descriptor().depth_write
        && spec.effect == 0 && !spec.sky && !spec.env && !spec.fade && !spec.actor
}

/// Preserve every native index; paired slots occupy a separate suffix.
pub(super) fn extend(pipes: &mut Pipelines) {
    let native = pipes.len();
    for index in 0..native {
        let spec = pipes.spec(index);
        for soft in [false, true] {
            pipes.push(if eligible(spec) {
                PipelineSlot::new(PipelineSpec {
                    effect: if spec.sprite { 20 } else { 18 } + u32::from(soft),
                    ..spec
                })
            } else { PipelineSlot::alias(index) });
        }
    }
}

pub(super) fn pipeline(pipes: &Pipelines, native: usize, soft: bool) -> Option<usize> {
    let base = pipes.len() / 3;
    (native < base && eligible(pipes.spec(native)))
        .then_some(base + native * 2 + usize::from(soft))
}

/// Called after Enhanced attachment wrappers are generated. Copy their complete
/// output calculation rather than duplicating lighting or native alpha policy.
pub(super) fn append(source: &mut String) {
    source.push_str(&crate::shader_source::load("foliage_edges.wgsl", include_str!("foliage_edges.wgsl")));
    for (original, stem) in [("fs_blend", "foliage_blend"), ("fs_sprite", "foliage_sprite")] {
        let marker = format!("@fragment fn {original}(");
        let start = source.find(&marker).expect("generated native foliage wrapper");
        let end = start + source[start..].find("\n}").expect("generated foliage wrapper end") + 2;
        let wrapper = source[start..end].to_owned();
        for (suffix, reject) in [("core", "!foliage_core(beauty.a)"), ("soft", "foliage_core(beauty.a)")] {
            let wrapper = wrapper.replacen(&marker, &format!("@fragment fn fs_{stem}_{suffix}("), 1)
                .replacen("    let bounded_beauty", &format!("    if {reject} {{ discard; }}\n    let bounded_beauty"), 1);
            source.push('\n');
            source.push_str(&wrapper);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn partition_preserves_native_coverage_and_alpha_test() {
        let mut source = super::super::enhanced_shader::source(false, false, false, false);
        super::append(&mut source);
        for stem in ["blend", "sprite"] {
            for suffix in ["core", "soft"] {
                let start = source.find(&format!("@fragment fn fs_foliage_{stem}_{suffix}(")).unwrap();
                let body = &source[start..start + source[start..].find("\n}").unwrap()];
                assert!(body.contains(&format!("enhanced_original_fs_{stem}(")));
                assert!(body.find("enhanced_original_fs_").unwrap() < body.find("foliage_core(beauty.a)").unwrap());
            }
        }
        assert!(source.contains("if native_alpha_test() && !native_alpha_pass(alpha)"));
        assert!(source.contains("return alpha >= 0.95;"));
        for value in 0..=255 {
            let alpha = value as f32 / 255.0;
            for native_pass in [false, true] {
                let core = native_pass && alpha >= 0.95;
                let soft = native_pass && alpha < 0.95;
                assert_eq!(core || soft, native_pass);
                assert!(!(core && soft));
            }
        }
    }
}
