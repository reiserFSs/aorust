//! Live wiring of `ao_formats::lightning` into the zone flow: ticks the native storm schedules every frame, turns
//! fired [`ao_formats::lightning::Strike`]s into transient additive fan quads (FX8's `VisualCloudlight_t`; see the
//! module doc of `ao_formats::lightning` for why these are not routed through `combat::effects` — FX8/FX9 have no
//! RDB template and are not bound to a dynel). `SM_Sandy_Env_NearLightning`'s `delay = distance / 100` s goes
//! through `ao_audio::Audio::play_lightning_cue`'s own delayed-command queue (see its doc for why that reuse is
//! faithful, not an approximation).

use std::sync::Arc;

use ao_formats::lightning::{self, Lightning, Strike};
use ao_formats::weather::CrtRand;
use ao_scene::{ActorFrame, Blend, Mesh, Scene, Submesh, Vertex, IDENTITY};

/// Model key of the shared 5-vertex fan mesh ([`fan_model`]); every strike's fans reuse it with a per-actor `skin`
/// override (dynamic world-space vertices, like `combat::effects`' own particle/mesh actors).
pub const FAN_MODEL: u64 = 0xfac3_0000_0000_0000;
/// High `id` range for fan actors, distinct from `combat::effects`' `ACTOR_BASE` (`0xe000_0000`) and real dynel ids.
const FAN_ACTOR_BASE: u32 = 0xd000_0000;

/// Model key of the shared 4-vertex bolt-segment quad ([`bolt_model`]); every segment of every bolt reuses it with
/// its own `skin`, like [`FAN_MODEL`].
pub const BOLT_MODEL: u64 = 0xb017_0000_0000_0000;
/// High `id` range for bolt-segment actors, distinct from [`FAN_ACTOR_BASE`] and `combat::effects`' `ACTOR_BASE`.
const BOLT_ACTOR_BASE: u32 = 0xc000_0000;
/// `10052182`: two perpendicular strips connecting adjacent four-corner sections.
pub const BOLT_INDICES: [u32; 12] = [0, 1, 4, 1, 5, 4, 2, 3, 6, 3, 7, 6];

/// The shared bolt-segment submesh: additive, untextured, two-sided, same reasoning as [`fan_model`].
pub fn bolt_model() -> Scene {
    let mut sub = Submesh::new(BOLT_INDICES.to_vec(), None);
    sub.blend = Blend::Additive;
    sub.two_sided = true;
    Scene { meshes: vec![Mesh { vertices: vec![Vertex::default(); 8], submeshes: vec![sub] }], ..Default::default() }
}

/// The shared fan submesh ([`lightning::FAN_INDICES`]): additive, untextured, two-sided (the native fan is a flat
/// quad meant to be seen from any angle while falling). Vertex positions are overridden per actor by [`LightningFx::actors`]'s `skin`.
pub fn fan_model() -> Scene {
    let mut sub = Submesh::new(lightning::FAN_INDICES.to_vec(), None);
    sub.blend = Blend::Additive;
    sub.two_sided = true;
    Scene { meshes: vec![Mesh { vertices: vec![Vertex::default(); 5], submeshes: vec![sub] }], ..Default::default() }
}

struct ActiveFan {
    id: u32,
    /// World positions of the 5 local vertices (`strike.source + Fan::vertices()`).
    verts: [[f32; 3]; 5],
    /// `fan_color`'s roll, applied to the center vertex (index 4); corners stay transparent.
    color: [u8; 3],
    /// `FAN_LIFETIME` counting down to `fan_expired`.
    life: f32,
}

struct ActiveBolt {
    id: u32,
    bolt: lightning::Bolt,
}

/// Per-zone lightning state: the native storm schedules plus the render/audio side this port adds.
pub struct LightningFx {
    engine: Lightning,
    fans: Vec<ActiveFan>,
    bolts: Vec<ActiveBolt>,
    /// Flicker/color RNG (`fan_color`/`flicker_alpha`'s own non-reproducible `CrtRand` draws, matching
    /// `lightning::additional_instance_fans`'s treatment of the same native non-determinism).
    rng: CrtRand,
    /// `FUN_100b0d1f`/`FUN_100b0f33`'s own free-running real-time clock, accumulated frame to frame.
    elapsed: f64,
    next_id: u32,
    model_uploaded: bool,
    bolt_model_uploaded: bool,
}

impl Default for LightningFx {
    fn default() -> Self {
        LightningFx { engine: Lightning::default(), fans: Vec::new(), bolts: Vec::new(), rng: CrtRand::default(), elapsed: 0.0, next_id: 0, model_uploaded: false, bolt_model_uploaded: false }
    }
}

impl LightningFx {
    /// One frame: advances the storms when `enabled` (`weather::State::lightning_enabled()`), ages and drops expired
    /// fans, and returns the strikes fired this frame (for the caller's own audio one-shot via
    /// `Strike::audio`/`ao_audio::Audio::play_lightning_cue`). `controlled` is the camera/player world position,
    /// `extent` is `ao_formats::lightning::extent(tile_size)` (`0.0` fires nothing), `camera_y` the camera's own Y.
    pub fn tick(&mut self, enabled: bool, dt: f32, controlled: [f32; 3], extent: f32, camera_y: f32) -> Vec<Strike> {
        self.elapsed += f64::from(dt);
        let strikes = if enabled { self.engine.advance(self.elapsed, controlled, extent, camera_y) } else { Vec::new() };
        for strike in &strikes {
            for fan in &strike.fans {
                let verts = fan.vertices().map(|[x, y, z]| [strike.source[0] + x, strike.source[1] + y, strike.source[2] + z]);
                let id = FAN_ACTOR_BASE | (self.next_id & 0x0fff_ffff);
                self.next_id = self.next_id.wrapping_add(1);
                self.fans.push(ActiveFan { id, verts, color: lightning::fan_color(&mut self.rng), life: lightning::FAN_LIFETIME });
            }
            for (source, ground) in strike.bolts() {
                let bolt = lightning::Bolt::new(source, ground, &mut self.rng);
                let id = BOLT_ACTOR_BASE | (self.next_id & 0x0fff_ffff);
                self.next_id = self.next_id.wrapping_add(bolt.points.len() as u32 - 1);
                self.bolts.push(ActiveBolt { id, bolt });
            }
        }
        for fan in &mut self.fans {
            fan.life -= dt;
        }
        for bolt in &mut self.bolts {
            bolt.bolt.advance(dt, &mut self.rng);
        }
        self.fans.retain(|f| !lightning::fan_expired(f.life));
        self.bolts.retain(|b| b.bolt.life > 0.0);
        strikes
    }

    /// This frame's actor model uploads, once per [`LightningFx`] instance (reset on zone load, matching
    /// `combat::effects`' own "upload once" `host.actor_models` precedent).
    pub fn take_model_upload(&mut self) -> Vec<(u64, std::sync::Arc<Scene>)> {
        let mut uploads = Vec::new();
        if !self.model_uploaded {
            self.model_uploaded = true;
            uploads.push((FAN_MODEL, std::sync::Arc::new(fan_model())));
        }
        if !self.bolt_model_uploaded {
            self.bolt_model_uploaded = true;
            uploads.push((BOLT_MODEL, std::sync::Arc::new(bolt_model())));
        }
        uploads
    }

    /// This frame's fan and bolt-segment quads, ready to push into `host.actors`.
    pub fn actors(&mut self) -> Vec<ActorFrame> {
        let Self { fans, bolts, rng, .. } = self;
        let fan_actors = fans.iter().map(|fan| {
            let alpha = lightning::flicker_alpha(rng);
            let center = [f32::from(fan.color[0]) / 255.0, f32::from(fan.color[1]) / 255.0, f32::from(fan.color[2]) / 255.0, f32::from(alpha) / 255.0];
            let verts: Vec<Vertex> =
                fan.verts.iter().enumerate().map(|(i, &pos)| Vertex { pos, color: if i == 4 { center } else { [0.0; 4] }, ..Default::default() }).collect();
            ActorFrame { id: fan.id, model: FAN_MODEL, transform: IDENTITY, skin: Some(Arc::new(verts)), always: true, ..Default::default() }
        });
        let bolt_actors = bolts.iter().flat_map(|active| {
            let n = active.bolt.points.len();
            active.bolt.points.windows(2).enumerate().map(move |(i, points)| {
                let verts = (0..8).map(|j| {
                    let section = j / 4;
                    let mut pos = points[section];
                    match j % 4 {
                        0 => pos[0] -= 0.5,
                        1 => pos[0] += 0.5,
                        2 => pos[2] -= 0.5,
                        _ => pos[2] += 0.5,
                    }
                    let alpha = if i + section == 0 || i + section == n - 1 { 0.0 } else { 1.0 };
                    Vertex { pos, color: [200.0 / 255.0, 1.0, 1.0, alpha], ..Default::default() }
                }).collect();
                ActorFrame { id: active.id + i as u32, model: BOLT_MODEL, transform: IDENTITY, skin: Some(Arc::new(verts)), always: true, ..Default::default() }
            })
        });
        fan_actors.chain(bolt_actors).collect()
    }

    /// Active fan count, for tests.
    #[cfg(test)]
    pub fn fan_count(&self) -> usize {
        self.fans.len()
    }

    /// Active bolt-segment count, for tests.
    #[cfg(test)]
    pub fn bolt_count(&self) -> usize {
        self.bolts.iter().map(|b| b.bolt.points.len() - 1).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_fires_nothing_even_with_a_valid_extent() {
        let mut fx = LightningFx::default();
        let mut fired = 0usize;
        for _ in 0..2000 {
            fired += fx.tick(false, 0.5, [0.0, 0.0, 0.0], 60.0, 5.0).len();
        }
        assert_eq!(fired, 0);
        assert_eq!(fx.fan_count(), 0);
        assert!(fx.actors().is_empty());
    }

    #[test]
    fn enabled_eventually_fires_and_renders_the_anchor_plus_nine_fans_per_strike() {
        let mut fx = LightningFx::default();
        let mut strikes = Vec::new();
        for _ in 0..2000 {
            strikes.extend(fx.tick(true, 0.5, [0.0, 0.0, 0.0], 60.0, 5.0));
            if !strikes.is_empty() {
                break;
            }
        }
        assert!(!strikes.is_empty(), "storm 1 (period 600 s) never fired over 1000 s");
        assert_eq!(fx.fan_count(), strikes.iter().map(|s| s.fans.len()).sum::<usize>(), "anchor + 9 fans queued per strike, nothing expired yet");
        assert!(fx.fan_count() >= 10, "anchor + 9 fans per strike should be queued");
        let actors = fx.actors();
        let fan_actors: Vec<_> = actors.iter().filter(|a| a.model == FAN_MODEL).collect();
        let bolt_actors: Vec<_> = actors.iter().filter(|a| a.model == BOLT_MODEL).collect();
        assert_eq!(actors.len(), fan_actors.len() + bolt_actors.len(), "every actor is a fan or a bolt segment");
        assert_eq!(fan_actors.len(), fx.fan_count());
        assert_eq!(bolt_actors.len(), fx.bolt_count());
        // FX9 lives less than 0.5s; this half-second step may already retire all bolts.
        for actor in &fan_actors {
            let skin = actor.skin.as_ref().unwrap();
            assert_eq!(skin.len(), 5);
            // corners transparent, center carries the flicker alpha
            for v in &skin[..4] {
                assert_eq!(v.color, [0.0; 4]);
            }
            assert!(skin[4].color[3] >= 0.0 && skin[4].color[3] <= 1.0);
        }
        for actor in &bolt_actors {
            let skin = actor.skin.as_ref().unwrap();
            assert_eq!(skin.len(), 8, "two crossed strips");
            assert!(skin.iter().all(|v| v.color[..3] == [200.0 / 255.0, 1.0, 1.0]));
        }
    }

    #[test]
    fn fans_expire_after_their_lifetime() {
        let mut fx = LightningFx::default();
        let mut strikes = Vec::new();
        for _ in 0..2000 {
            strikes.extend(fx.tick(true, 0.5, [0.0, 0.0, 0.0], 60.0, 5.0));
            if !strikes.is_empty() {
                break;
            }
        }
        assert!(fx.fan_count() > 0);
        fx.tick(false, lightning::FAN_LIFETIME + 0.1, [0.0, 0.0, 0.0], 60.0, 5.0);
        assert_eq!(fx.fan_count(), 0, "fans must expire after FAN_LIFETIME");
    }

    #[test]
    fn bolt_cross_sections_keep_native_colour_and_endpoint_alpha() {
        let mut fx = LightningFx::default();
        let bolt = lightning::Bolt::new([0.0, 30.0, 0.0], [0.0, -40.0, 0.0], &mut CrtRand::default());
        let n = bolt.points.len();
        fx.bolts.push(ActiveBolt { id: BOLT_ACTOR_BASE, bolt });
        let actors = fx.actors();
        assert_eq!(actors.len(), n - 1);
        for (i, actor) in actors.iter().enumerate() {
            let vertices = actor.skin.as_ref().unwrap();
            for (j, v) in vertices.iter().enumerate() {
                assert_eq!(v.color[..3], [200.0 / 255.0, 1.0, 1.0]);
                assert_eq!(v.color[3], if i + j / 4 == 0 || i + j / 4 == n - 1 { 0.0 } else { 1.0 });
            }
            assert!((vertices[1].pos[0] - vertices[0].pos[0] - 1.0).abs() < 1e-6);
            assert!((vertices[3].pos[2] - vertices[2].pos[2] - 1.0).abs() < 1e-6);
        }
        fx.tick(false, 0.5, [0.0; 3], 60.0, 0.0);
        assert!(fx.actors().is_empty());
    }

    #[test]
    fn model_upload_happens_once_for_each_of_the_two_models() {
        let mut fx = LightningFx::default();
        let first = fx.take_model_upload();
        assert_eq!(first.len(), 2, "fan model + bolt model");
        assert!(first.iter().any(|(k, _)| *k == FAN_MODEL));
        assert!(first.iter().any(|(k, _)| *k == BOLT_MODEL));
        assert!(fx.take_model_upload().is_empty());
    }

    /// Deterministic offscreen render of a fired strike's fans + bolts (main bolt always, companion bolts per
    /// `Strike::companions`): `AOMAC_SHOT_DIR=/tmp/x cargo test --release -p aomac -- play::lightning::offscreen_strike_frame`
    /// -> `lightning_strike.png`.
    #[test]
    fn offscreen_strike_frame() -> anyhow::Result<()> {
        let Some(out) = std::env::var_os("AOMAC_SHOT_DIR").map(std::path::PathBuf::from) else { return Ok(()) };
        std::fs::create_dir_all(&out)?;
        let mut fx = LightningFx::default();
        let mut strikes = Vec::new();
        for _ in 0..2000 {
            strikes.extend(fx.tick(true, 0.5, [0.0, 0.0, 0.0], 60.0, 5.0));
            if !strikes.is_empty() {
                break;
            }
        }
        assert!(!strikes.is_empty(), "storm 1 never fired");
        let models = fx.take_model_upload();
        assert_eq!(models.len(), 2);
        let actors = fx.actors();
        assert!(!actors.is_empty());
        let source = strikes[0].source;
        let eye = [source[0] + 60.0, source[1] + 20.0, source[2] + 60.0];
        ao_render::render_to_png_actors(&ao_scene::Scene::default(), &models, actors, eye, source, 640, 480, &out.join("lightning_strike.png"), 0.0)?;
        Ok(())
    }

    /// Leak regression (user report: "massive memory leak... had to hard reset"): a long-lived `Renderer` (same
    /// object for the whole loop, like the real windowed session) fed a real pf 4582 scene plus continuous night
    /// lightning (fan/bolt actor churn every frame, the exact per-frame GPU path `flow.rs` drives). Root cause: with
    /// `MouseLagFix` off (the default), nothing ever polled the `wgpu::Device`, so every buffer/texture/bind group
    /// replaced that frame (actor skins here; combat-effect particles, sky and liquid geometry too in the live app)
    /// sat as a pending GPU destruction wgpu only reclaims on a poll -- confirmed by comparing an unpolled half
    /// (~1.05 KB/frame RSS growth) against a polled half (~0.05 KB/frame, 23x less) of this same loop. The fix
    /// (`ao_render::viewer`'s render loop) now runs a non-blocking `device.poll(Poll)` every frame unconditionally;
    /// this regression exercises that same recipe directly and must stay flat.
    #[test]
    #[ignore = "real GPU + client data; run explicitly: cargo test --release -p aomac -- --ignored lightning_render_loop_rss_stays_flat --nocapture"]
    fn lightning_render_loop_rss_stays_flat() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let dir = std::path::PathBuf::from(home).join("Games/ProjectRubiKa/client");
        if !dir.join("cd_image/rdb.db").exists() { return; }
        let store = ao_rdb::RecordStore::open(&dir).unwrap();
        let (scene, _report) = ao_formats::playfield::load_playfield_report_at(&store, &dir, 4582, 0.0).unwrap();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut r = ao_render::Renderer::new(&instance, None).unwrap();
        r.upload(&scene);
        let (w, h) = (640u32, 480u32);
        let targets = ao_render::Targets::new(&r, w, h);
        let out = r.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: r.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = out.create_view(&Default::default());
        let mut fx = LightningFx::default();
        let eye = [scene.spawn.map_or(0.0, |p| p[0]), scene.spawn.map_or(50.0, |p| p[1] + 10.0), scene.spawn.map_or(0.0, |p| p[2])];
        let cam = ao_render::Camera::look_at(eye.into(), [eye[0] + 20.0, eye[1], eye[2] + 20.0].into());
        let rss = |pid: u32| -> u64 {
            let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &pid.to_string()]).output().unwrap();
            String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0)
        };
        let pid = std::process::id();
        let mut samples = Vec::new();
        const FRAMES: usize = 18000; // 5 simulated minutes at 60 fps, run as fast as possible
        for i in 0..FRAMES {
            fx.tick(true, 1.0 / 60.0, eye, 200.0, eye[1]);
            for (key, model) in fx.take_model_upload() { r.add_actor_model(key, &model); }
            r.set_actors(fx.actors());
            r.render(&view, &targets, &cam);
            let _ = r.device.poll(wgpu::PollType::Poll); // the fix: reclaim this frame's destroyed GPU resources now
            if i % 1500 == 0 { samples.push((i, rss(pid))); }
        }
        samples.push((FRAMES, rss(pid)));
        eprintln!("rss samples (frame, KB): {samples:?}");
        let (warm, last) = (samples[2].1 as f64, samples.last().unwrap().1 as f64); // skip the first two (startup allocations)
        let growth = last - warm;
        let per_frame = growth / (FRAMES - 3000) as f64;
        eprintln!("growth after warm-up: {growth:.0} KB over {} frames ({per_frame:.2} KB/frame)", FRAMES - 3000);
        assert!(growth < 20_000.0, "RSS grew {growth:.0} KB ({per_frame:.2} KB/frame) over {} frames after warm-up with the device polled every frame: the leak is back", FRAMES - 3000);
    }
}
