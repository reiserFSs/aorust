//! Opt-in, aomac-only graphics settings. All effects default off.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnhancedSettings {
    pub sun_shadows: bool,
    pub ambient_occlusion: bool,
    pub emissive_bloom: bool,
    pub ground_grass: bool,
    /// HDR scene + tonemap/grade/dither present; brightens only daylight-sun-lit surfaces.
    pub hdr: bool,
    pub shadow_map_size: u32,
    /// Share of the sun that full shadow removes (the rest stays as sky-lit shade).
    pub shadow_strength: f32,
    /// Penumbra growth per blocker distance; 0 is the hard 3x3 PCF.
    pub shadow_softness: f32,
    pub ao_radius: f32,
    pub ao_strength: f32,
    pub bloom_strength: f32,
    pub grass_density: f32,
    pub grass_distance: f32,
    /// Peak extra brightness of a fully sunlit, unshadowed, unfogged daylight surface (0 = faithful).
    pub sun_boost: f32,
    /// Chroma-only grade strength inside the HDR present (luminance-preserving; 0 = off).
    pub grade_strength: f32,
}

impl Default for EnhancedSettings {
    fn default() -> Self {
        Self {
            sun_shadows: false, ambient_occlusion: false, emissive_bloom: false, ground_grass: false, hdr: false,
            shadow_map_size: 2048, shadow_strength: 0.65, shadow_softness: 1.5, ao_radius: 1.0, ao_strength: 1.0, bloom_strength: 0.6,
            grass_density: 1.0, grass_distance: 50.0, sun_boost: 0.35, grade_strength: 0.7,
        }
    }
}

impl EnhancedSettings {
    pub fn any_enabled(self) -> bool {
        self.sun_shadows || self.ambient_occlusion || self.emissive_bloom || self.ground_grass || self.hdr
    }

    /// Bound external preferences, replacing non-finite floats with their defaults.
    pub fn clamped(self) -> Self {
        fn finite(value: f32, max: f32, default: f32) -> f32 {
            if value.is_finite() { value.clamp(0.0, max) } else { default }
        }
        let defaults = Self::default();
        Self {
            shadow_map_size: self.shadow_map_size.clamp(256, 4096),
            shadow_strength: finite(self.shadow_strength, 1.0, defaults.shadow_strength),
            shadow_softness: finite(self.shadow_softness, 4.0, defaults.shadow_softness),
            ao_radius: finite(self.ao_radius, 5.0, defaults.ao_radius),
            ao_strength: finite(self.ao_strength, 1.0, defaults.ao_strength),
            bloom_strength: finite(self.bloom_strength, 1.0, defaults.bloom_strength),
            grass_density: finite(self.grass_density, 2.0, defaults.grass_density),
            grass_distance: finite(self.grass_distance, 60.0, defaults.grass_distance),
            sun_boost: finite(self.sun_boost, 0.6, defaults.sun_boost),
            grade_strength: finite(self.grade_strength, 1.0, defaults.grade_strength),
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_flags_and_quality_bounds() {
        let defaults = EnhancedSettings::default();
        assert!(!defaults.any_enabled());
        assert_eq!(defaults.bloom_strength, 0.6);
        assert_eq!(defaults.clamped(), defaults);
        for mask in 0..32 {
            let settings = EnhancedSettings {
                sun_shadows: mask & 1 != 0, ambient_occlusion: mask & 2 != 0,
                emissive_bloom: mask & 4 != 0, ground_grass: mask & 8 != 0, hdr: mask & 16 != 0,
                ao_strength: 0.0, bloom_strength: 0.0, grass_density: 0.0, sun_boost: 0.0, grade_strength: 0.0, ..defaults
            };
            assert_eq!(settings.any_enabled(), mask != 0);
        }
        assert_eq!(EnhancedSettings { sun_boost: 9.0, ..defaults }.clamped().sun_boost, 0.6);
        let bounded = EnhancedSettings {
            shadow_map_size: 0, ao_radius: -1.0, ao_strength: 3.0, bloom_strength: 2.0,
            grass_density: 4.0, grass_distance: 100.0, ..defaults
        }.clamped();
        assert_eq!((bounded.shadow_map_size, bounded.ao_radius, bounded.ao_strength), (256, 0.0, 1.0));
        assert_eq!((bounded.bloom_strength, bounded.grass_density, bounded.grass_distance), (1.0, 2.0, 60.0));
        assert_eq!(defaults.shadow_map_size, 2048);
        let shadow = EnhancedSettings { shadow_strength: 2.0, shadow_softness: 9.0, ..defaults }.clamped();
        assert_eq!((shadow.shadow_strength, shadow.shadow_softness), (1.0, 4.0));
        assert_eq!(EnhancedSettings { shadow_map_size: u32::MAX, ..defaults }.clamped().shadow_map_size, 4096);
        for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(EnhancedSettings {
                shadow_strength: invalid, shadow_softness: invalid, ao_radius: invalid, ao_strength: invalid, bloom_strength: invalid,
                grass_density: invalid, grass_distance: invalid, sun_boost: invalid, grade_strength: invalid, ..defaults
            }.clamped(), defaults);
        }
    }
}
