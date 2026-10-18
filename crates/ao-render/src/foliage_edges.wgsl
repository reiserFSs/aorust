// Enhanced-only partition of the native shaded alpha. Native discard has already
// run; these complementary predicates do not remove any surviving coverage.
fn foliage_core(alpha: f32) -> bool {
    return alpha >= 0.95;
}
