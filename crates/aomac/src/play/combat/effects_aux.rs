//! Native non-geometric impact dependencies: GroundShake (3032) and Audio (4000).
//! GC dispatch 100ce4f7; shake ctor/load/process 1010ed0d/1010eaa3/1010ebd8;
//! audio ctor/load/process 100d3190/100d30c1/100d3012. Neither creates a DS quad.
//! N3 1001ff2f consumes camera+1d4 through LocalityListener+130: visual eye
//! translation only, no rotation or movement. SI 100071ed consumes the sound
//! command as volume/radius/duration/delay, material0/size1. Impact71345 is the
//! supported zero-duration/zero-delay/zero-velocity one-shot; other timing or
//! velocity is explicitly rejected rather than silently discarded.
use super::{EffectConfig, Template};
use anyhow::{ensure, Result};
use ao_formats::weather::R250;
use ao_rdb::RecordStore;
use glam::{Mat4, Vec3};

// Gamecode.dll VA 102c3f28, fourteen 64-byte names (not RDB texture selectors).
const SOUND_NAMES: [&str; 14] = [
    "SM_Sandy_Buff_Ping", "SM_Sandy_Debuff_Ping", "SM_Sandy_Tower_Beam",
    "SM_Sandy_Tower_Hit", "SM_Sandy_Tower_Fire_Start", "SM_Sandy_Tower_Fire",
    "SM_Sandy_Tower_Creation_Basic", "SM_Sandy_Tower_Creation_Superior",
    "SM_Sandy_Tower_Destruction_Basic", "SM_Sandy_Tower_Destruction_Superior",
    "SM_Sandy_Tower_Powerdown", "SM_Sandy_Game_Explo_small",
    "SM_Sandy_Game_Explo_Med", "SM_Sandy_Game_Explo_Big",
];

/// Exact AFCM Send(0x1a,0x103,pos,velocity), SetData(f32 x4), SetData(id,probability).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AuxSound {
    pub id: u32,
    pub pos: [f32; 3],
    pub velocity: [f32; 3],
    /// SI100071ed: volume, radius, duration, delay (authored words4..7).
    pub parameters: [f32; 4],
    pub probability: i32,
}

pub struct AuxEffect {
    template: Template,
    source: Mat4,
    elapsed: f32,
    duration: f32,
    envelope: Vec<[f32; 2]>,
    sound: Option<AuxSound>,
    processed: bool,
    stopped: bool,
}

impl AuxEffect {
    pub fn supports(kind: i32) -> bool { matches!(kind, 3032 | 4000) }

    pub fn new(t: &Template, source: Mat4, _target: Vec3, config: EffectConfig, _store: &RecordStore) -> Result<Self> {
        Self::parse(t, source, config)
    }

    fn parse(t: &Template, source: Mat4, config: EffectConfig) -> Result<Self> {
        ensure!(Self::supports(t.kind), "unsupported auxiliary class {}", t.kind);
        let mut envelope = Vec::new();
        let (duration, sound) = if t.kind == 3032 {
            for i in 1..=12 { t.float(i)?; }
            let count = t.word(13)? as usize;
            ensure!(count <= t.words.len().saturating_sub(14) / 2, "truncated ground shake envelope");
            ensure!(t.words.len() == 14 + count * 2, "trailing ground shake parameters");
            for i in 0..count { envelope.push([t.float(14 + i * 2)?, t.float(15 + i * 2)?]); }
            ensure!(t.float(12)? > 0.0, "invalid ground shake range");
            let duration = config.duration.unwrap_or(t.float(8)?);
            ensure!(duration > 0.0 && duration.is_finite(), "invalid ground shake duration");
            (duration, None)
        } else {
            ensure!(t.words.len() == 9, "invalid audio effect payload length");
            let selector = t.word(0)? as usize;
            let name = SOUND_NAMES.get(selector).ok_or_else(|| anyhow::anyhow!("unknown native audio selector {selector}"))?;
            let velocity = [t.float(1)?, t.float(2)?, -t.float(3)?];
            let parameters = [t.float(4)?.clamp(0.0, 1.0), t.float(5)?, t.float(6)?, t.float(7)?];
            // Only the one-shot contract is currently routed; never discard authored timing/velocity.
            ensure!(velocity == [0.0; 3] && parameters[2] == 0.0 && parameters[3] == 0.0,
                "audio effect requires unported duration/delay/velocity: {:?}, {:?}", parameters, velocity);
            ensure!((0.0..=65535.0).contains(&parameters[1]), "invalid authored audio radius");
            (0.0, Some(AuxSound { id: ao_audio::sbf::sound_id(name), pos: source.w_axis.truncate().to_array(), velocity, parameters, probability: t.word(8)? as i32 }))
        };
        Ok(Self { template: t.clone(), source, elapsed: 0.0, duration, envelope, sound, processed: false, stopped: false })
    }

    pub fn update_source(&mut self, source: Mat4) {
        self.source = source;
        if let Some(sound) = &mut self.sound { sound.pos = source.w_axis.truncate().to_array(); }
    }
    pub fn next_state(&mut self) { self.stopped = true; self.sound = None; }
    pub fn advance(&mut self, dt: f32) -> bool {
        self.elapsed += dt;
        self.processed = true;
        !self.stopped && self.template.kind == 3032 && self.elapsed <= self.duration
    }
    pub fn take_sound(&mut self) -> Option<AuxSound> {
        if self.processed { self.sound.take() } else { None }
    }

    /// Native camera +1d4..1dc is overwritten, not accumulated, by each active shake.
    /// The caller must sample in effect order and use the last Some offset this frame.
    pub fn camera_offset(&self, eye: Vec3, rng: &mut R250) -> Result<Option<Vec3>> {
        if self.template.kind != 3032 || self.stopped || self.elapsed > self.duration { return Ok(None); }
        let point = super::sprites::connector(&self.template, self.source)?.w_axis.truncate();
        let range = self.template.float(12)?;
        let gain = (range - eye.distance(point).min(range)) / range * envelope_at(&self.envelope, self.elapsed / self.duration);
        let mut draw = || ((rng.next_f64() * 2.0 - 1.0) * gain as f64) as f32;
        Ok(Some(Vec3::new(draw(), draw(), -draw())))
    }
}

// GC1011634c returns the final knot outside every interval, including before the first.
fn envelope_at(knots: &[[f32; 2]], t: f32) -> f32 {
    for pair in knots.windows(2) {
        if pair[0][0] <= t && t < pair[1][0] {
            let width = pair[1][0] - pair[0][0];
            let fraction = (t - pair[0][0]) / if width == 0.0 { 1.0 } else { width };
            return pair[1][1] * fraction + (1.0 - fraction) * pair[0][1];
        }
    }
    knots.last().map_or(1.0, |k| k[1])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authored_impact_auxiliary_dependencies() {
        let shake = Template { kind: 3032, words: vec![3,0,0,0,0,0,0,0,0x40000000,0,0,0,0x41a00000,2,0,0x3f000000,0x3f800000,0] };
        let mut effect = AuxEffect::parse(&shake, Mat4::IDENTITY, EffectConfig::default()).unwrap();
        assert_eq!(envelope_at(&effect.envelope, 0.5), 0.25);
        let mut rng = R250::new(1);
        let sample = effect.camera_offset(Vec3::ZERO, &mut rng).unwrap().unwrap();
        assert!(sample.abs().max_element() <= 0.5 && sample != Vec3::ZERO);
        assert_eq!(effect.camera_offset(Vec3::X * 21.0, &mut rng).unwrap(), Some(Vec3::ZERO));
        assert!(effect.advance(2.0));
        assert!(!effect.advance(0.001));
        let sound = Template { kind: 4000, words: vec![12,0,0,0,0x3f800000,0x42f00000,0,0,100] };
        let mut effect = AuxEffect::parse(&sound, Mat4::from_translation(Vec3::new(1.0,2.0,3.0)), EffectConfig::default()).unwrap();
        assert!(effect.take_sound().is_none());
        assert!(!effect.advance(0.01));
        let event = effect.take_sound().unwrap();
        assert_eq!(event.id, ao_audio::sbf::sound_id("SM_Sandy_Game_Explo_Med"));
        assert_eq!(event.pos, [1.0,2.0,3.0]);
        assert_eq!(event.parameters, [1.0,120.0,0.0,0.0]);
        assert_eq!(event.probability, 100);
        assert!(effect.take_sound().is_none());
        let mut delayed = sound; delayed.words[7] = 1.0f32.to_bits();
        assert!(AuxEffect::parse(&delayed, Mat4::IDENTITY, EffectConfig::default()).is_err());
        let mut truncated = shake; truncated.words.pop();
        assert!(AuxEffect::parse(&truncated, Mat4::IDENTITY, EffectConfig::default()).is_err());
    }
}
