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
(`T_native`, DESIGN section 8). Post (`post.rs`, `post.wgsl`): exposure, shoulder, grade, vignette, dither (spatial-only: stable per
output pixel, no frame/time term), 2.2-gamma encode.

## Pyramid and AO (slot 3)

`pyramid.rs/.wgsl` (`hiz`): the one depth pyramid, R32Float min VIEW depth, mips 0..7 (mip 0 = linearised view depth, `1e9`
where the G-buffer has no geometry); two dispatches, `Pyramid::view()` exposes all mips. `ao.rs/.wgsl` (`ao`, `ao_denoise`,
`ao_up`): half-res GTAO (2 slices x 6 steps per side, mip by sample distance, 1.5 m radius, thin-occluder compensation, edge-on
surfaces fade to 1), 3x3 depth-bilateral blur + temporal blend (history reprojected with the unjittered matrices and looked up through
the previous frame's jitter, positions reconstructed with the current jitter undone, half-res texel centre = 2x2 block centre,
depth-rejected, dropped on resize / drawn-actor hash change / TAA jitter toggle), then a depth-aware upsample into the full-res `R8Unorm`
texture at resolve binding 6 (`flags.y` bit 0). AO only multiplies the hemisphere ambient term. Sky and fully fogged texels are 1
and never act as occluders (`native_transmittance(z) > 0`). The sampling pattern only rotates through time when TAA jitters, so
identical frames give bit-identical AO. `ao_up` is a raster pass (R8Unorm cannot be a storage target without extra features): its
timestamp overlaps neighbouring passes on Metal, read the `frame` row for totals.

FX colour space (canopy speckle root cause): the blended remainder (RSprite/alpha-blended leaf cards, glass, glows) is authored for
D3D's gamma-space blend. Blended in the linear HDR target, every partial-alpha leaf-edge texel is lifted towards white: an outline of
bright isolated pixels around each leaf (1.9x Original, `newland-sun`; TAA and AO were not involved: identical with `AOMAC_V2_NO_TAA`,
and the leaf cards never reach the G-buffer). The resolve therefore writes the HDR target gamma-encoded (`hdr_encode`, resolve.wgsl),
the FX wrappers return the native gamma colour/alpha unchanged, and the TAA input tile decodes (`pow 2.2`); no extra pass. CAS runs
at `CAS_GAIN` 0.5 of the setting (full gain re-sharpened the canopy edges: +13% speckle). Regression:
`graphics_v2_canopy_edges_are_no_noisier_than_original` (speckle <= 1.3x Original, canopy frame-to-frame < 1.5 LSB).
The `fx` pass on Newland (0.41 ms) vs ICC (0.25 ms) is the canopy: hundreds of overlapping full-resolution blended cards, no AO/TAA work.

Post stack (`motion.rs`, `bloom.rs`, `taa.rs`, `post.rs`, WGSL `motion/bloom/taa/post/post_common.wgsl`):
- Motion (timer `motion`, only when actors are drawn): admitted opaque/alpha-tested actor items are redrawn with hardware `Equal`
  against the G-buffer depth and write displayed-pose motion into G2 (previous model + previous displayed vertices per pose; module is
  nested in `actors.rs` for the private actor layer). G2 = 0 means "static: reproject from depth", >= 32 means "no history".
- Bloom (timer `bloom`): half-res, 5-level dual filter, threshold-free, seeded ONLY by G3 emission x native transmittance behind a soft
  knee (0.12..0.45 on the brightest channel, linear): the Newland lamp pole keeps its native look, the head glows. The halo is added in
  HDR before TAA by the TAA input stage at `EnhancedSettings::bloom_strength` (default 0.6), attenuated by the receiving pixel's
  `T_native` (a fully fogged pixel never changes).
- TAA (timer `taa`): Halton-8 jitter; the jittered render is reconstructed on the UNJITTERED output grid (bilinear at pixel + jitter,
  12x12 tile with a 2-pixel halo, variance bounds from the reconstructed neighbours), catmull-rom history, 3x3 variance clip,
  depth-dilated motion, ping-pong RGBA16F history whose alpha is the sample count (blend weight 1/n, n <= 32, capped near 3 while moving,
  restarted by a rejected history). History is discarded (`taa::reset_reason`) on frame 0/resize, camera cut (> 5 m or > 25 deg per
  frame) and any change of the drawn-actor hash (`admission::drawn_set_hash`: order-independent SUM of mixed `(actor, mesh)` hashes of the
  post-cull actor items, so duplicates never cancel and undrawn host frames never churn it). `AOMAC_V2_NO_TAA=1` (read once at start)
  disables jitter and accumulation for before/after stills. `AOMAC_V2_TEMPORAL_TRACE=1` (read once, zero cost when unset) logs per frame
  on stderr: frame, time, dt, eye, forward, jitter, drawn-actor hash/item count and whether/why the TAA and AO histories reset.
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
split / fog end, `horizon_fade`). A static layer is fitted once (1.5 / 4 / 12 m of slack, moon 6 m) and then keeps its matrix: it is
refitted only when the camera's frustum slice leaves it (moon at most 4 Hz) or the light is more than 0.05 degrees away from the light
it was fitted with (the stored light only changes on a refit, so slow sun drift accumulates until it refits). A caster change inside it
(content signature; the cheap world hash is checked every frame) or a changed scene time while it holds time-animated casters
(scrolling alpha, wind) re-renders it every time with the SAME matrix, growing only the depth range for newly streamed casters. The
dynamic (actor) layer is rendered and sampled with the covering static cascade's matrix (same snap, never refitted per frame).
`AOMAC_V2_TEMPORAL_TRACE=1` logs the per-frame cache decision (reason, refit, matrix hash, dynamic matrix) per layer. Diagnostics for the harness:
`AOMAC_V2_SHADOWS=off` (no shadow work), `AOMAC_V2_SHADOW_RERENDER=<layer bit mask>` (force static re-renders every frame).
Not yet in the cascades: grass blade and canopy-leaf casters, contact shadows, cloud shadows (later milestones).

## Harness

`AOMAC_GFX_SETTINGS=v2-high AOMAC_GFX_VIEW=<view> AOMAC_GFX_OUT=<new dir> AOMAC_GFX_PNG_ONLY=1 cargo test --profile fast -p aomac
graphics_offline_fixture -- --ignored --nocapture` (GPU lock first, see the aorust skill). Add `AOMAC_GFX_FRAME_TIMING=1` for
120 warm-up + 600 sampled frames with per-pass GPU timestamps (`<view>-v2-passes.csv/.txt`, p50/p95 per pass and `frame`).
Views: `icc-landmark-authored`, `newland-sun` (look-target camera), `newland-day`, `borealis-street-*`.

Temporal stability (flicker/shaking): `AOMAC_GFX_TEMPORAL_REGRESSION=1 AOMAC_GFX_SETTINGS=v2-high AOMAC_GFX_VIEW=icc-landmark-authored
AOMAC_GFX_OUT=<new dir> cargo test --profile fast -p aomac graphics_temporal_stability -- --ignored --nocapture` (GPU lock first). Renders
64 warm + 24 measured consecutive frames for Original and the preset in the same run, in four modes: frozen camera, +-1 mm camera
micro-motion, animated scene time (dt jitter 4..20 ms; other modes pass dt 0, so effect time is explicit), all without an actor
(steady state), and `churn` = frozen camera with a real actor shown/hidden every 8 frames (the toggle pair is excluded). Per mode it prints
`TEMPORAL mode= preset= whole= shadow= edge= worst=` and writes `temporal-<mode>-<preset>.csv` (mean abs RGB delta overall, in shadow
pixels = non-edge pixels < 0.7x their 17x17 mean, in Sobel-edge pixels, and worst single channel). Frozen, micro and churn v2 must stay
<= max(1.2 x Original, 0.25) whole and shadow, <= max(1.2 x Original, 1.0) edge; frozen and micro also worst <= max(1.2 x Original, 64).
Churn worst is unbounded: an admission change resets history by fairness design, so that frame is the raw jittered one. A failure keeps
`temporal-<mode>-<preset>-worst-a/b.png`. `AOMAC_GFX_TEMPORAL_PROBE=x,y` prints `PROBE mode preset frame r g b` per measured frame.

### Diagnostics (real window, all read once, zero cost when unset)

- `AOMAC_FRAME_DUMP=<absolute dir>,<N>` writes N **consecutive presented** frames of the final swapchain colour (after the GUI, read
  back before present; the dump blocks on each readback) as `frame_000000.png`... Use `/tmp/<you>/...`; a relative dir is rejected.
  `AOMAC_FRAME_DUMP_DELAY=<seconds>` waits that long after Play reports the world playing (character in the world, loading screen
  gone; `ao_render::set_world_playing`, called by the Play loop every frame) before the first frame.
- `AOMAC_FREEZE_CAMERA=1` renders from the camera captured on the first playing frame (no idle follow motion); compare a frozen
  dump against a normal one, and against Original, to separate camera motion from renderer flicker.
- Ctrl+Shift+O (`AomacEffectGpuOverlay`) and `AomacSlowFrameLog` now show v2: while the live profiler is on the v2 pass timers
  (`shadows, gbuffer, hiz, ao, ao_denoise, ao_up, resolve, fx, bloom, taa, post, frame`, as in the fixture) feed its 120-frame
  rolling costs from the oldest ring slot (3 frames back); spans are non-exclusive (see M2). Off = no queries, no readback.
- `AOMAC_PERF=1` also prints `frame_ms avg/p95/worst` every 5 s; slow frames (>50 ms) log `total` wall ms.

## M2 measurements (High, 1920x1200, M4 Pro, p50 of 600 samples after 120 warm-up, GPU lock held)

Whole frame (first pass begin to last end), `AOMAC_V2_OFF` unset: ICC 2.02 ms, Newland 2.30 ms (design total 5.7 ms).

On Metal tile GPUs a pass's vertex work overlaps the previous pass's fragment work, so per-pass timestamp **spans** are
non-exclusive and do not sum to the frame. The trustworthy per-pass cost is the **marginal frame delta**: frame p50 with the pass
switched off minus frame p50 with it on (`AOMAC_V2_OFF=<ao|bloom|cas|cube|admission|shadows>`, comma list, read once;
`AOMAC_V2_OFF=post2` encodes post twice; images are not meaningful while a pass is off). Measured under the GPU lock,
600 samples after 120 warm-up, noise about +-0.03 ms. Neither view draws an actor, so admission and the dynamic shadow layers are
idle here (with an actor: shadows +0.26..0.35 ms, M2 numbers above).

| Pass | DESIGN row (ms) | Span ICC / Newland | Marginal delta ICC / Newland | Note |
|---|---|---|---|---|
| gbuffer | 1.10 | 0.36 / 0.39 | not switchable | span is the pass |
| shadows (cascades + moon, + admission) | 0.35 | none (label removed) | 0.06 / 0.02 | steady state, static layers cached |
| admission | (in shadows) | none | 0.02 / 0.00 | no actors in the view |
| hiz | 0.09 | 0.054 / 0.054 | not switchable (AO input) | span is the pass |
| ao + ao_denoise + ao_up | 0.83 | 0.30+0.13 / 0.28+0.12; `ao_up` 0.91 / 0.90 | **0.48 / 0.41** | `ao_up` is a raster span that overlaps neighbours; use the delta |
| cube atlas (`cube`) | 0.40 (was 0.25; raised by decision 2026-10-20) | 0.10 / 0.22; about 0.35 with a carried light | 0.03 / 0.10 | atlas skipped once resident. With a carried light the actor's bounding sphere reaches all 6 cube faces and v1 encodes one face pass per face per frame, about 50 us each (6 x 50 us = 0.30 ms plus the cache); only the GPU knows which faces rebuild, so the CPU cannot skip them safely. Accepted: whole frames are 2.0-2.6 ms against 5.7 |
| clusters | 0.05 | 0.005 / 0.012 | not switchable | span is the pass |
| resolve (incl. shadow/lamp taps) | 0.74 | 0.19 / 0.28 | shadows taps: 0.05 / 0.05 | |
| fx | (0.20 + water) | 0.25 / 0.41 | not switchable | canopy cards on Newland |
| bloom | 0.18 | 0.105 / 0.106 | 0.13 / 0.12 | |
| taa (incl. history resolve) | 0.55 | 0.56 / 0.56 | not switchable (post reads its history) | span is the pass |
| cas | (in taa/post) | none | 0.01 / -0.01 (noise) | |
| post (grade, vignette, dither) | (in taa row) | not reliable | 0.00..0.03 (post2 delta 0.016 / -0.027: below noise) | |

`post`: its timestamp span cannot be made reliable on Metal (the end sample reads about 5 us with or without a depth attachment, and
as the start of the following marker it overlaps the preceding pass), so the `post` row is no longer printed as a span. Encoding post
twice moves the frame by less than the +-0.03 ms noise, i.e. the pass costs under about 0.03 ms at 1920x1200 on this GPU, and it
does not meet a "> 0.05 ms" plausibility check because it genuinely is that cheap (full-screen, one tap, tile-resident). The post
pass now has a read-only depth attachment (it also helps the TAA/post path reading the same depth).

## Review #1 (real window, only while Main allows it)

Options -> Enhanced -> Presets -> Graphics Preset -> "Enhanced v2 High" (preference `AomacGraphicsPreset = 5`; building the
pipelines takes a few seconds on selection). Stand at the ICC arrival beach (the `icc-landmark` camera: x 933, z -748 looking
toward the platform) and in Newland City on the street at x 414, z -320 (look toward the sun, yaw -90 deg, pitch +20 deg); day only.
Look at: tree/lamp/pole shadows on grass and the platform shadow on the beach (no shimmer when turning), contact darkening at
building bases and under the ICC overhang (no halos), stable edges while walking (TAA), cyan signs and lamp heads glowing, the lamp
pole staying dull, and a carried Light Bar lighting the ground with the actor's shadow. Not in M2 (later milestones): authored
sky/cloud rework, froxel haze/shafts, water material, grass/foliage in the G-buffer, night look, per-zone grade.
