# Enhanced v2

One designed deferred renderer (`crates/ao-render/src/v2/`) selected as a graphics preset next to the untouched faithful
path. Design, budgets and fairness rules: `~/aomac-handoff/v2/DESIGN.md` (approved 2026-10-19; first slice = ICC + Newland).
v1 Enhanced (`Modern`/`Ultra` and their ~115 `Aomac*` keys) stays until the user accepts the slice.

## Entry and faithful guarantee

`Renderer::render` takes one branch at its top when `EnhancedSettings::enhanced_v2` is set (preset **Enhanced v2 High**,
`AomacGraphicsPreset = 5`) and no native screen layers are active. Everything below the branch is the faithful code; the only
edits to it are pure extractions (`frame_globals`, `cull_frame`, `sort_draws`, `update_sky_frame`, `draw_actors_with`,
`Pipelines::get`) shared with v2. Gate on every commit: baseline mode of `graphics_offline_fixture` must report
`baseline_differing_bytes=0` for Original (MSAA 1) and Classic+ (MSAA 4) on `icc-landmark-authored`,
`borealis-street-sun-high`, `borealis-street-night`, `newland-day`.

## Frame graph (implemented)

`[shadows] -> G-buffer -> [AO] -> resolve (compute) -> FX (blended draws, forward into HDR) -> [TAA/bloom] -> post`

G-buffer (`gbuffer.rs`, `gbuffer.wgsl`, `packing.wgsl`): the native vertex stages, draw ordering, instancing and material bind
groups are reused; two pipeline tables are built from the native ones (`g`: opaque/alpha-test slots + sky layers, `fx`: every
blended slot). Targets: sky colour RGBA16F, G0 RGBA8 (gamma albedo + flags), G1 RGB10A2 (octahedral normal + roughness/aux),
G2 RG16F (motion), G3 RG11B10F (authored emission), D32F depth. The sky never writes depth.

Resolve (`resolve.rs`, `resolve.wgsl`): sun, hemisphere ambient, emission, native linear fog applied analytically
(`T_native`, DESIGN section 8). Post (`post.rs`, `post.wgsl`): exposure, shoulder, grade, vignette, dither, 2.2-gamma encode.

## Pyramid and AO (slot 3)

`pyramid.rs/.wgsl` (`hiz`): the one depth pyramid, R32Float min VIEW depth, mips 0..7 (mip 0 = linearised view depth, `1e9`
where the G-buffer has no geometry); two dispatches, `Pyramid::view()` exposes all mips. `ao.rs/.wgsl` (`ao`, `ao_denoise`,
`ao_up`): half-res GTAO (2 slices x 6 steps per side, mip by sample distance, 1.5 m radius, thin-occluder compensation, edge-on
surfaces fade to 1), 3x3 depth-bilateral blur + temporal blend (history reprojected with the unjittered matrices, depth-rejected,
dropped on resize / admitted-actor hash change / TAA jitter toggle), then a depth-aware upsample into the full-res `R8Unorm`
texture at resolve binding 6 (`flags.y` bit 0). AO only multiplies the hemisphere ambient term. Sky and fully fogged texels are 1
and never act as occluders (`native_transmittance(z) > 0`). The sampling pattern only rotates through time when TAA jitters, so
identical frames give bit-identical AO. `ao_up` is a raster pass (R8Unorm cannot be a storage target without extra features): its
timestamp overlaps neighbouring passes on Metal, read the `frame` row for totals.

## Harness

`AOMAC_GFX_SETTINGS=v2-high AOMAC_GFX_VIEW=<view> AOMAC_GFX_OUT=<new dir> AOMAC_GFX_PNG_ONLY=1 cargo test --profile fast -p aomac
graphics_offline_fixture -- --ignored --nocapture` (GPU lock first, see the aorust skill). Add `AOMAC_GFX_FRAME_TIMING=1` for
120 warm-up + 600 sampled frames with per-pass GPU timestamps (`<view>-v2-passes.csv/.txt`, p50/p95 per pass and `frame`).
Views: `icc-landmark-authored`, `newland-sun` (look-target camera), `newland-day`, `borealis-street-*`.
