//! `GAME.GameWaveCurve0..15` and the sun-ray flicker table: the per-frame sin/cos waves the game writes into the shared
//! `VisualEnvFX` struct (Gamecode `FUN_100b005c` @0x100b005c, called every frame from `FUN_100b19e4` with the frame time).
//! DisplaySystem copies them to the `GAME` tweak variables (`FUN_1005cdcf` @0x1005cdcf: struct `+0x90..+0xcc` -> `GameWaveCurve0..15`)
//! and `FUN_1005a3a9` (`e_SunRays`) reads the integer table at struct `+0x50`.

/// Phase (degrees) added to the phase accumulator by each curve (the constants of `FUN_100b005c`: 90, 180, 0, 270, 45, 135,
/// 350, 50, 100 (@0x10158670), 0, 45, 270, 36, 112, 350, 50).
pub const WAVE_PHASE: [f32; 16] = [90.0, 180.0, 0.0, 270.0, 45.0, 135.0, 350.0, 50.0, 100.0, 0.0, 45.0, 270.0, 36.0, 112.0, 350.0, 50.0];

/// Degrees the phase accumulator advances per second (`dt * 50`, double @0x10155a68).
pub const WAVE_DEGREES_PER_SECOND: f64 = 50.0;

/// `GameWaveCurve0..15` after `time` seconds of game frames. The client keeps `s = fmod(s + dt * 50, 360)` (@0x1013ed9c) and
/// stores `0.5 + 0.5 * cos((s + phase) * 3.14 / 180)` (`cos` = `_CIcos` @0x1013efb8, constants 3.14 @0x10167458, 180 @0x10163a40,
/// 0.5 @0x10155f00): one period is 7.2 s. The initial `s` (a member of the object `FUN_100b005c` runs on) is taken as 0.
#[allow(clippy::approx_constant)] // the client's literal 3.14, not pi
pub fn wave_curves(time: f32) -> [f32; 16] {
    let s = (time as f64 * WAVE_DEGREES_PER_SECOND) % 360.0;
    WAVE_PHASE.map(|p| {
        let a = ((s as f32 + p) as f64 * 3.14 / 180.0) as f32;
        0.5 + 0.5 * a.cos()
    })
}

/// The 8 entries the sun-ray fan (`FUN_1005a3a9`) reads: struct `+0x50 + 4 i` = `trunc(255 * GameWaveCurve_i)` (`FUN_100b005c`,
/// 255.0 @0x10166f60, `FUN_1013ecf0` is the truncating float-to-int); entries 8..15 are never used by the sun (its selector at
/// `+0x32c` is only ever initialised to 0, so the low half is read).
pub fn sun_flicker_table(time: f32) -> [u32; 8] {
    let c = wave_curves(time);
    std::array::from_fn(|i| (c[i] * 255.0) as u32)
}

/// A sky instance that additionally turns about `axis` (right-handed scene space, unit) through the point `pivot` (relative to
/// the camera) by `degrees * GameWaveCurve<curve>(time)`: a tweak rotation `v(axis), GAME.GameWaveCurveN * k + c` whose constant
/// part is baked into the instance (the renderer composes this turn before the instance transform every frame).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyWaveSpin {
    /// Index into [`crate::Scene::sky`].
    pub instance: usize,
    pub axis: [f32; 3],
    pub pivot: [f32; 3],
    pub curve: usize,
    pub degrees: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::approx_constant)] // the client's literal 3.14
    fn curves_are_half_plus_half_cos_of_the_phase() {
        let c = wave_curves(0.0);
        assert!((c[2] - 1.0).abs() < 1e-6, "phase 0 -> 1");
        assert!(c[1] < 1e-5, "phase 180 -> 0");
        assert!((c[0] - 0.5).abs() < 1e-3, "phase 90 -> 0.5");
        // curve 7 at 1 s: phase 50 + 50 = 100 degrees
        let want = 0.5 + 0.5 * (100.0f64 * 3.14 / 180.0).cos();
        assert!((wave_curves(1.0)[7] as f64 - want).abs() < 1e-5);
    }

    #[test]
    fn period_is_7_2_seconds() {
        for (a, b) in wave_curves(1.3).iter().zip(wave_curves(1.3 + 7.2).iter()) {
            assert!((a - b).abs() < 1e-3);
        }
    }

    #[test]
    fn flicker_table_truncates_255_times_the_curve() {
        let t = sun_flicker_table(0.0);
        assert_eq!(t[2], 255);
        assert_eq!(t[1], 0);
        assert_eq!(t[0], 127); // 0.50040 * 255 = 127.6 truncated
        assert!(sun_flicker_table(0.7).iter().all(|&v| v <= 255));
    }
}
