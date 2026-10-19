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

Post stack (`motion.rs`, `bloom.rs`, `taa.rs`, `post.rs`, WGSL `motion/bloom/taa/post/post_common.wgsl`):
- Motion (timer `motion`, only when actors are drawn): admitted opaque/alpha-tested actor items are redrawn with hardware `Equal`
  against the G-buffer depth and write displayed-pose motion into G2 (previous model + previous displayed vertices per pose; module is
  nested in `actors.rs` for the private actor layer). G2 = 0 means "static: reproject from depth", >= 32 means "no history".
- Bloom (timer `bloom`): half-res, 5-level dual filter, threshold-free, seeded ONLY by G3 emission x native transmittance behind a soft
  knee (0.12..0.45 on the brightest channel, linear): the Newland lamp pole keeps its native look, the head glows. The halo is added in
  HDR before TAA by the TAA input stage at `EnhancedSettings::bloom_strength` (default 0.6), attenuated by the receiving pixel's
  `T_native` (a fully fogged pixel never changes).
- TAA (timer `taa`): Halton-8 jitter, catmull-rom history, 3x3 variance clip, depth-dilated motion, ping-pong RGBA16F history. History
  is discarded (`taa::history_reset`) on frame 0/resize, camera cut (> 5 m or > 25 deg per frame) and any change of the admitted-actor
  hash. `AOMAC_V2_NO_TAA=1` (read once at start) disables jitter and accumulation for before/after stills.
- Post: CAS (`EnhancedSettings::cas_strength`, skipped for `T_native < 0.15`), then the grade; saturation and contrast fade to identity for
  `T_native < 0.5` so the grade cannot recover fogged contrast (`post::tests::grade_is_contrast_neutral_over_fog`).
- Regressions (`play/v2_glow.rs`, ignored, GPU lock): lamp pole <= 1.25x Original and head halo; admission flip leaves no history residue
  (byte-identical frames 2-3); a walking actor leaves no trail; a static camera does not flicker.
Admission (`admission.rs`, slot 1, label `admission`): after the G-buffer only this frame's actors (alpha > 1e-5) are drawn into an
R32Uint id target depth-tested against the G-buffer depth, then reduced to `Admission::visibility()`, a fixed 8193-word storage
array indexed by the actor interaction slot (layout frozen, documented at the top of the file). It is read by the dynamic shadow
layers and by the lamps' carried-light owner check; a zero-alpha actor has no item, no slot and is never admitted. Frames without
actors encode nothing.

Shadows (`shadows.rs` + `shadows.wgsl`, slot 2, label `shadows`; resolve plugin 0 = group 1): v1's `CascadeFrame` (3 texel-snapped
cascades, 2048^2) plus Moon A (one 1024^2 cascade, when `moon_dir.w` is set). Every cascade has a STATIC layer (world casters only,
cached; sampled with the matrix it was rendered with) and a DYNAMIC layer (admitted actors only, cleared every frame, 1024^2 for the
sun); the receiver takes the `min` of both 3x3 PCF taps (slope-scaled bounded bias, 10% cascade crossfade, fade to lit at the last
split / fog end, `horizon_fade`). A static layer is re-rendered only when the camera's frustum slice leaves it (each is fitted
with 1.5 / 4 / 12 m of slack, moon 6 m and at most 4 Hz), the light moves more than 0.05 degrees, or a caster inside it changes
(content signature; the cheap world hash is checked every frame). Time-animated casters refresh at 4 Hz. Diagnostics for the harness:
`AOMAC_V2_SHADOWS=off` (no shadow work), `AOMAC_V2_SHADOW_RERENDER=<layer bit mask>` (force static re-renders every frame).
Not yet in the cascades: grass blade and canopy-leaf casters, contact shadows, cloud shadows (later milestones).

## Harness

`AOMAC_GFX_SETTINGS=v2-high AOMAC_GFX_VIEW=<view> AOMAC_GFX_OUT=<new dir> AOMAC_GFX_PNG_ONLY=1 cargo test --profile fast -p aomac
graphics_offline_fixture -- --ignored --nocapture` (GPU lock first, see the aorust skill). Add `AOMAC_GFX_FRAME_TIMING=1` for
120 warm-up + 600 sampled frames with per-pass GPU timestamps (`<view>-v2-passes.csv/.txt`, p50/p95 per pass and `frame`).
Views: `icc-landmark-authored`, `newland-sun` (look-target camera), `newland-day`, `borealis-street-*`.
