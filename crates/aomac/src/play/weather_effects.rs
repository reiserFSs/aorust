//! Authored native smoke/sparkle sources for the opt-in weather acceptance fixture.
use anyhow::{Context, Result, ensure};
use glam::{Mat4, Vec3};
use std::path::Path;
use crate::play::combat::effects::{Binding, EffectConfig, Renderer, Templates};

pub(in super::super) struct NativeEffect {
    renderer: Renderer,
    handle: u32,
    record: i32,
    kind: i32,
    material: u32,
    rate: f32,
    life: f32,
    duration: f32,
    position: Vec3,
    replays: u32,
    elapsed: f32,
}
impl NativeEffect {
    pub(in super::super) fn new(dir: &Path, kind: i32, position: Vec3) -> Result<Self> {
        ensure!(matches!(kind, 1009 | 1018 | 3017), "capture source must be native Smoke1009, Sparks1018 or GlobalSmoke3017");
        let templates = Templates::open(dir)?;
        // Select real authored, sustained, visible emitters. Never edit a template, colour,
        // material, emission rate, lifetime or duration to manufacture an acceptance source.
        let mut candidates: Vec<_> = templates.by_id.iter().filter_map(|(&id, template)| {
            if template.kind != kind { return None; }
            let word = |i| template.words.get(i).copied();
            let float = |i| word(i).map(f32::from_bits);
            let duration = float(8)?;
            let rate = if kind == 3017 { word(30)? as f32 } else { float(10)? };
            let life = float(if kind == 1018 { 35 } else { 26 })?;
            let width = float(if kind == 1018 { 12 } else { 14 })?;
            let height = float(if kind == 1018 { 14 } else { 16 })?;
            let alpha = float(if kind == 1018 { 16 } else { 18 })?;
            if ![duration, rate, life, width, height, alpha].iter().all(|v| v.is_finite())
                || !(1.0..=256.0).contains(&rate)
                || !(0.5..=12.0).contains(&life) || !(0.05..=4.0).contains(&width)
                || !(0.05..=4.0).contains(&height) || alpha <= 0.0 { return None; }
            if kind == 3017 && word(10)? == 1 { return None; } // Mode1 requires a real terrain contact provider.
            if kind == 1018 && !(17..=19).any(|index| float(index).is_some_and(|v| v > 0.0)) { return None; }
            Some((id, word(9)?, rate, life, duration))
        }).collect();
        candidates.sort_unstable_by_key(|candidate| (kind == 3017 && candidate.0 != 12252, candidate.0));
        let mut renderer = Renderer::open(dir)?;
        let mut failures = Vec::new();
        for (record, material, rate, life, duration) in candidates {
            let binding = Binding { group: 0, attractor: 0, effect: record, note: 0, color: 0 };
            match renderer.spawn_configured(binding, Mat4::from_translation(position), position, EffectConfig::default()) {
                Ok(handle) if handle != 0 && renderer.is_active(handle) => {
                    return Ok(Self { renderer, handle, record, kind, material, rate, life, duration, elapsed: 0.0, position, replays: 0 });
                }
                Ok(_) => failures.push(format!("{record}: native constructor returned null")),
                Err(error) => failures.push(format!("{record}: {error:#}")),
            }
        }
        anyhow::bail!("no viable authored sustained effect class {kind}: {}", failures.join("; "))
    }
    pub(in super::super) fn frame(&mut self, dt: f32, host: &mut ao_render::Host) -> Result<()> {
        ensure!(dt.is_finite() && dt >= 0.0, "invalid native effect fixture step");
        ensure!(self.renderer.is_active(self.handle), "authored effect {} retired before capture", self.record);
        self.renderer.frame(dt, host, None);
        self.elapsed += dt;
        let replayed = !self.renderer.is_active(self.handle);
        if replayed {
            // Replay the unmodified authored control, as repeated native effect events do.
            // Do not stretch its duration/lifetime to match the capture window.
            self.handle = self.renderer.spawn_configured(Binding { group: 0, attractor: 0, effect: self.record, note: 0, color: 0 },
                Mat4::from_translation(self.position), self.position, EffectConfig::default())?;
            ensure!(self.handle != 0, "authored effect replay returned null");
            self.replays += 1;
            self.renderer.frame(0.0, host, None);
        }
        if self.elapsed >= 0.25 && !replayed {
            host.actors.iter().find(|actor| actor.id == self.handle && actor.enhanced_emitter.is_some())
                .with_context(|| format!("authored effect {} did not submit native emitter metadata", self.record))?;
        }
        Ok(())
    }
    pub(in super::super) fn metadata(&self) -> String {
        format!("gfxtweak record={} class={} material={} native_rate={} native_life={} native_duration={} native_elapsed={} authored_replays={}",
            self.record, self.kind, self.material, self.rate, self.life, self.duration, self.elapsed, self.replays)
    }
}
