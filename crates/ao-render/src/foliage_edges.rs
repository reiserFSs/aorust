//! Enhanced two-pass foliage with blended, non-depth-writing silhouette coverage.
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

/// Called after Enhanced attachment wrappers are generated. Reuse their complete
/// output calculation; only paired leaf silhouettes relax hard alpha thresholds.
pub(super) fn append(source: &mut String) {
    source.push_str(&crate::shader_source::load("foliage_edges.wgsl", include_str!("foliage_edges.wgsl")));
    // A native GREATER/GREATEREQUAL alpha test otherwise hard-cuts the very
    // edge this pass is meant to soften. Only paired plant draws use this
    // generated copy; unusual alpha comparisons and every other shader stay native.
    let start = source.find("fn shade(").expect("native shaded foliage");
    let end = start + source[start..].find("\n}").expect("native shade end") + 2;
    let shade = source[start..end].replacen("fn shade(", "fn foliage_edge_shade(", 1)
        .replace(
            "if native_alpha_test() && !native_alpha_pass(alpha) {",
            "if native_alpha_test() && select(!native_alpha_pass(alpha), alpha <= 0.0, u32(mat.native.w) == 5u || u32(mat.native.w) == 7u) {",
        );
    source.push('\n');
    source.push_str(&shade);
    for (original, stem) in [("fs_blend", "foliage_blend"), ("fs_sprite", "foliage_sprite")] {
        let marker = format!("@fragment fn {original}(");
        let start = source.find(&marker).expect("generated native foliage wrapper");
        let end = start + source[start..].find("\n}").expect("generated foliage wrapper end") + 2;
        let mode = if original == "fs_sprite" { 6 } else { 2 };
        let mut wrapper = source[start..end].to_owned();
        for input in ["i", "surface"] {
            wrapper = wrapper.replace(&format!("enhanced_original_{original}({input})"), &format!("foliage_edge_shade({input}, {mode}u)"));
        }
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
    fn partition_softens_greater_tests_without_touching_native_shader() {
        let mut source = super::super::enhanced_shader::source(crate::EnhancedSettings::default());
        let native_shade = source.split("fn shade(").nth(1).unwrap().split("\n}").next().unwrap().to_owned();
        super::append(&mut source);
        for stem in ["blend", "sprite"] {
            for suffix in ["core", "soft"] {
                let start = source.find(&format!("@fragment fn fs_foliage_{stem}_{suffix}(")).unwrap();
                let body = &source[start..start + source[start..].find("\n}").unwrap()];
                assert!(body.contains("foliage_edge_shade("));
                assert!(body.find("foliage_edge_shade(").unwrap() < body.find("foliage_core(beauty.a)").unwrap());
            }
        }
        assert!(source.contains("if native_alpha_test() && !native_alpha_pass(alpha)"));
        assert!(source.contains("return alpha >= 0.95;"));
        assert_eq!(source.split("fn shade(").nth(1).unwrap().split("\n}").next().unwrap(), native_shade);
        let edge = source.split("fn foliage_edge_shade(").nth(1).unwrap().split("\n}").next().unwrap();
        assert!(edge.contains("alpha <= 0.0, u32(mat.native.w) == 5u || u32(mat.native.w) == 7u"));
        for value in 0..=255 {
            let alpha = value as f32 / 255.0;
            let native_pass = alpha > 0.5;
            let soft_pass = alpha > 0.0 && alpha < 0.95;
            if alpha > 0.0 && !native_pass {
                assert!(soft_pass, "fractional silhouette coverage must reach the non-depth-writing pass");
            }
            if alpha == 0.0 { assert!(!soft_pass); }
        }
        for value in 0..=255 {
            let alpha = value as f32 / 255.0;
            for shaded_pass in [false, true] {
                let core = shaded_pass && alpha >= 0.95;
                let soft = shaded_pass && alpha < 0.95;
                assert_eq!(core || soft, shaded_pass);
                assert!(!(core && soft));
            }
        }
    }
}
