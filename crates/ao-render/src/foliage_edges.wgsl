// Enhanced-only partition of the native shaded alpha. Native discard has already
// run; these complementary predicates do not remove any surviving coverage.
fn foliage_core(alpha: f32) -> bool {
    return alpha >= 0.95;
}

// Both eligible entrypoints use native mode 2 (sprite mode 6 maps to 2).
// Moon/geometric receivers change lighting only, never these alpha inputs.
fn foliage_alpha(i: VOut) -> f32 {
    let material_alpha = select(mat.color.a, i.fade, i.fade < 1.0);
    let texture_alpha = select(early_primary.a, 1.0, mat.emissive.w > 0.5 && mat.emissive.w < 1.5);
    return texture_alpha * i.color.a * material_alpha;
}
