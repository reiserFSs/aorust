//! Opt-in, aomac-only graphics settings. All effects default off.

/// Independent night aesthetics; the default retains the authored game lighting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum NightMode {
    #[default]
    AsGame,
    Moonlit,
    Dark,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnhancedSettings {
    pub sun_shadows: bool,
    pub ambient_occlusion: bool,
    pub emissive_bloom: bool,
    pub ground_grass: bool,
    pub grass_tufts: bool,
    pub grass_species: bool,
    pub grass_path_thinning: bool,
    pub grass_gust_bands: bool,
    pub grass_sunlight: bool,
    pub grass_sun_shadows: bool,
    pub foliage_edges: bool,
    pub foliage_sway: bool,
    pub foliage_push: bool,
    pub foliage_leaves: bool,
    pub foliage_flutter: bool,
    pub foliage_falling: bool,
    pub grass_trails: bool,
    pub foliage_subdivision: bool,
    pub foliage_translucency: bool,
    pub foliage_lod: bool,
    /// HDR scene + tonemap/grade/dither present; daylight sun and mild outdoor sky fill.
    pub hdr: bool,
    pub grading_enabled: bool,
    pub zone_lut: bool,
    pub vignette: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub warmth: f32,
    pub night_cool: f32,
    pub taa: bool,
    pub fxaa: bool,
    pub cas: bool,
    pub cas_strength: f32,
    pub motion_blur: f32,
    pub depth_of_field: bool,
    pub dof_aperture: f32,
    pub dof_max_radius: f32,
    /// Half-resolution screen-space march towards the directional light; darkening only.
    pub contact_shadows: bool,
    pub height_haze: bool,
    pub land_haze: bool,
    pub valley_mist: bool,
    pub volumetric_shafts: bool,
    pub cloud_gap_shafts: bool,
    pub volumetric_clouds: bool,
    pub cloud_shadows: bool,
    pub sky_discs: bool,
    pub material_specular: bool,
    pub generated_normals: bool,
    pub material_maps: bool,
    pub material_pbr: bool,
    pub material_reflections: bool,
    pub environment_probe: bool,
    pub clustered_lighting: bool,
    pub enhanced_rain: bool,
    pub wet_surfaces: bool,
    pub weather_particles: bool,
    pub gpu_spell_particles: bool,
    pub fluid_smoke: bool,
    pub moon_shadows: bool,
    pub local_light_shadows: bool,
    pub point_shadow_count: u32,
    pub lamp_intensity: f32,
    pub carried_light_intensity: f32,
    pub night_mode: NightMode,
    pub moon_intensity: f32,
    pub dark_night_strength: f32,
    pub water_waves: bool,
    pub water_ripples: bool,
    pub water_refraction: bool,
    pub water_reflections: bool,
    pub water_foam: bool,
    pub water_wakes: bool,
    pub waterfalls: bool,
    pub underwater: bool,
    pub water_caustics: bool,
    pub water_sun_glint: bool,
    /// 0 keeps the richer native colour; 1 favours physically restrained water.
    pub water_realism: f32,
    pub water_wake_strength: f32,
    pub water_wake_visibility: f32,
    pub water_wake_duration: f32,
    pub water_wake_speed: f32,
    pub water_wake_treading: f32,
    pub water_wake_splash: f32,
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
    /// Daylight exposure control; peak extra gamma-space brightness is 60% of this (0 = faithful).
    pub sun_boost: f32,
    /// Chroma-only grade strength inside the HDR present (luminance-preserving; 0 = off).
    pub grade_strength: f32,
    /// Peak share of directional light a fully contact-shadowed pixel loses.
    pub contact_strength: f32,
}

impl Default for EnhancedSettings {
    fn default() -> Self {
        Self {
            sun_shadows: false, ambient_occlusion: false, emissive_bloom: false, ground_grass: false, hdr: false, contact_shadows: false,
            grass_tufts: false, grass_species: false, grass_path_thinning: false, grass_gust_bands: false,
            grass_sunlight: false, grass_sun_shadows: false,
            height_haze: false, land_haze: false, valley_mist: false, volumetric_shafts: false,
            cloud_gap_shafts: false, volumetric_clouds: false, cloud_shadows: false, sky_discs: false,
            material_specular: false, clustered_lighting: false, moon_shadows: false, local_light_shadows: false,
            generated_normals: false, material_maps: false, material_pbr: false,
            material_reflections: false, environment_probe: false,
            point_shadow_count: 4,
            lamp_intensity: 1.0, carried_light_intensity: 1.0,
            night_mode: NightMode::AsGame, moon_intensity: 1.0, dark_night_strength: 0.6,
            enhanced_rain: false, wet_surfaces: false, weather_particles: false,
            gpu_spell_particles: false, fluid_smoke: false,
            grading_enabled: false, zone_lut: false, vignette: 0.0, contrast: 1.0, saturation: 1.0, warmth: 0.5,
            night_cool: 0.0,
            taa: false, fxaa: false, cas: false, cas_strength: 0.5, motion_blur: 0.0, depth_of_field: false, dof_aperture: 2.0, dof_max_radius: 8.0,
            foliage_edges: false, foliage_sway: false, foliage_push: false, foliage_leaves: false,
            foliage_flutter: false, foliage_falling: false, grass_trails: false,
            foliage_subdivision: false, foliage_translucency: false, foliage_lod: false,
            water_waves: false, water_ripples: false, water_refraction: false, water_reflections: false,
            water_foam: false, water_wakes: false, waterfalls: false, underwater: false,
            water_caustics: false, water_sun_glint: false, water_realism: 0.5,
            water_wake_strength: 2.0, water_wake_visibility: 2.0, water_wake_duration: 0.3,
            water_wake_speed: 3.6, water_wake_treading: 1.0, water_wake_splash: 1.0,
            shadow_map_size: 2048, shadow_strength: 0.45, shadow_softness: 1.5, ao_radius: 1.75, ao_strength: 1.5, bloom_strength: 0.6,
            grass_density: 1.0, grass_distance: 160.0, sun_boost: 0.6, grade_strength: 0.8, contact_strength: 0.8,
        }
    }
}

impl EnhancedSettings {
    pub fn any_enabled(self) -> bool {
        self.sun_shadows || self.ambient_occlusion || self.emissive_bloom || self.ground_grass || self.needs_grade() || self.contact_shadows
            || self.height_haze || self.land_haze || self.valley_mist || self.volumetric_shafts
            || self.cloud_gap_shafts || self.volumetric_clouds || self.cloud_shadows || self.sky_discs
            || self.material_specular || self.clustered_lighting || self.moon_shadows || self.local_light_shadows
            || self.generated_normals || self.material_maps || self.material_pbr
            || self.material_reflections || self.environment_probe
            || self.night_mode != NightMode::AsGame || self.foliage_enabled() || self.post_enabled()
            || self.water_enabled()
            || self.enhanced_rain || self.wet_surfaces || self.weather_particles
            || self.gpu_spell_particles || self.fluid_smoke
            || self.grass_tufts || self.grass_species || self.grass_path_thinning || self.grass_gust_bands
            || self.grass_sunlight || self.grass_sun_shadows
    }

    /// Enhanced local-light consumers require current-frame owner admission before any source cap.
    pub(crate) fn needs_admitted_lights(self) -> bool {
        self.clustered_lighting || self.local_light_shadows || self.material_specular || self.material_pbr
            || self.material_reflections || self.environment_probe
            || self.generated_normals || self.material_maps
    }

    pub fn water_enabled(self) -> bool {
        self.water_waves || self.water_ripples || self.water_refraction || self.water_reflections
            || self.water_foam || self.water_wakes || self.waterfalls || self.underwater
            || self.water_caustics || self.water_sun_glint || self.enhanced_rain
    }

    pub fn foliage_enabled(self) -> bool {
        self.foliage_edges || self.foliage_sway || self.foliage_push || self.foliage_leaves
            || self.foliage_flutter || self.foliage_falling || self.grass_trails
            || self.foliage_subdivision || self.foliage_translucency || self.foliage_lod
    }

    pub fn interaction_enabled(self) -> bool {
        self.foliage_push || self.foliage_subdivision || self.grass_trails || self.ground_grass
    }

    pub fn needs_grade(self) -> bool {
        self.hdr || self.grading_enabled || self.zone_lut || self.vignette > 0.0
            || self.contrast != 1.0 || self.saturation != 1.0 || self.warmth != 0.5 || self.night_cool > 0.0
    }

    pub fn post_enabled(self) -> bool {
        self.taa || self.fxaa || self.cas || self.motion_blur > 0.0 || self.depth_of_field
    }

    /// Directional share, native fog fraction and authoritative admitted-world coverage (alpha); native sky/cloud depth is not world geometry.
    pub fn needs_sun_mask(self) -> bool {
        self.needs_grade() || self.contact_shadows || self.post_enabled()
            || self.cloud_shadows || self.volumetric_shafts || self.cloud_gap_shafts
            || self.height_haze || self.land_haze || self.valley_mist || self.sky_discs
            || self.enhanced_rain || self.wet_surfaces || self.weather_particles
            || self.gpu_spell_particles || self.fluid_smoke
            || self.material_pbr || self.material_reflections || self.environment_probe
    }

    /// Bound external preferences, replacing non-finite floats with their defaults.
    pub fn clamped(self) -> Self {
        fn finite(value: f32, max: f32, default: f32) -> f32 {
            if value.is_finite() { value.clamp(0.0, max) } else { default }
        }
        let defaults = Self::default();
        Self {
            shadow_map_size: self.shadow_map_size.clamp(256, 4096),
            point_shadow_count: self.point_shadow_count.clamp(1, 16),
            lamp_intensity: finite(self.lamp_intensity, 4.0, defaults.lamp_intensity),
            carried_light_intensity: finite(self.carried_light_intensity, 4.0, defaults.carried_light_intensity),
            moon_intensity: finite(self.moon_intensity, 4.0, defaults.moon_intensity),
            dark_night_strength: finite(self.dark_night_strength, 1.0, defaults.dark_night_strength),
            shadow_strength: finite(self.shadow_strength, 1.0, defaults.shadow_strength),
            shadow_softness: finite(self.shadow_softness, 4.0, defaults.shadow_softness),
            ao_radius: finite(self.ao_radius, 5.0, defaults.ao_radius),
            ao_strength: finite(self.ao_strength, 2.0, defaults.ao_strength),
            bloom_strength: finite(self.bloom_strength, 1.0, defaults.bloom_strength),
            grass_density: finite(self.grass_density, 2.0, defaults.grass_density),
            grass_distance: finite(self.grass_distance, 180.0, defaults.grass_distance),
            sun_boost: finite(self.sun_boost, 0.6, defaults.sun_boost),
            grade_strength: finite(self.grade_strength, 1.0, defaults.grade_strength),
            contact_strength: finite(self.contact_strength, 1.0, defaults.contact_strength),
            vignette: finite(self.vignette, 1.0, defaults.vignette),
            contrast: finite(self.contrast, 2.0, defaults.contrast),
            saturation: finite(self.saturation, 2.0, defaults.saturation),
            warmth: finite(self.warmth, 1.0, defaults.warmth),
            night_cool: finite(self.night_cool, 1.0, defaults.night_cool),
            cas_strength: finite(self.cas_strength, 1.0, defaults.cas_strength),
            motion_blur: finite(self.motion_blur, 1.0, defaults.motion_blur),
            dof_aperture: finite(self.dof_aperture, 8.0, defaults.dof_aperture),
            dof_max_radius: finite(self.dof_max_radius, 16.0, defaults.dof_max_radius),
            water_realism: finite(self.water_realism, 1.0, defaults.water_realism),
            water_wake_strength: finite(self.water_wake_strength, 4.0, defaults.water_wake_strength),
            water_wake_visibility: finite(self.water_wake_visibility, 4.0, defaults.water_wake_visibility),
            water_wake_duration: finite(self.water_wake_duration, 2.0, defaults.water_wake_duration).max(0.05),
            water_wake_speed: finite(self.water_wake_speed, 8.0, defaults.water_wake_speed).max(0.1),
            water_wake_treading: finite(self.water_wake_treading, 3.0, defaults.water_wake_treading),
            water_wake_splash: finite(self.water_wake_splash, 3.0, defaults.water_wake_splash),
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texture_material_switches_are_independent_and_default_off() {
        let defaults = EnhancedSettings::default();
        assert!(!defaults.generated_normals && !defaults.material_maps && !defaults.material_pbr);
        assert!(!defaults.material_reflections && !defaults.environment_probe);
        for mask in 0..32 {
            let settings = EnhancedSettings {
                generated_normals: mask & 1 != 0, material_maps: mask & 2 != 0,
                material_pbr: mask & 4 != 0, material_reflections: mask & 8 != 0,
                environment_probe: mask & 16 != 0, ..defaults
            };
            assert_eq!(settings.any_enabled(), mask != 0);
            assert_eq!(settings.needs_admitted_lights(), mask != 0);
            assert_eq!(settings.needs_sun_mask(), mask & 28 != 0);
            assert_eq!(settings.clamped(), settings);
        }
    }

    #[test]
    fn grass_switches_are_independent_and_default_off() {
        let defaults = EnhancedSettings::default();
        macro_rules! check {
            ($($field:ident),+ $(,)?) => {$(
                assert!(!defaults.$field);
                let settings = EnhancedSettings { $field: true, ..defaults };
                assert!(settings.any_enabled());
                assert!(!settings.ground_grass && !settings.sun_shadows);
                assert!(!settings.needs_sun_mask() && !settings.interaction_enabled());
                assert_eq!(settings.clamped(), settings);
            )+};
        }
        check!(grass_tufts, grass_species, grass_path_thinning, grass_gust_bands,
            grass_sunlight, grass_sun_shadows);
    }

    #[test]
    fn post_controls_are_off_independent_and_bounded() {
        let defaults = EnhancedSettings::default();
        for settings in [
            EnhancedSettings { grading_enabled: true, ..defaults },
            EnhancedSettings { zone_lut: true, ..defaults },
            EnhancedSettings { vignette: 0.5, ..defaults },
            EnhancedSettings { contrast: 1.5, ..defaults }, EnhancedSettings { saturation: 1.5, ..defaults },
            EnhancedSettings { warmth: 0.8, ..defaults }, EnhancedSettings { night_cool: 0.8, ..defaults },
        ] {
            assert!(!settings.hdr && settings.needs_grade() && settings.needs_sun_mask() && settings.any_enabled());
        }
        for settings in [
            EnhancedSettings { taa: true, ..defaults }, EnhancedSettings { fxaa: true, ..defaults },
            EnhancedSettings { cas: true, ..defaults }, EnhancedSettings { motion_blur: 0.5, ..defaults },
            EnhancedSettings { depth_of_field: true, ..defaults },
        ] { assert!(settings.post_enabled() && settings.any_enabled() && !settings.needs_grade()); }
        assert!(!EnhancedSettings { cas_strength: 1.0, ..defaults }.any_enabled());
        for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(EnhancedSettings { vignette: invalid, contrast: invalid, saturation: invalid, warmth: invalid, night_cool: invalid,
                cas_strength: invalid, motion_blur: invalid, dof_aperture: invalid, dof_max_radius: invalid, ..defaults }.clamped(), defaults);
        }
        let bounded = EnhancedSettings { vignette: 9.0, contrast: 9.0, saturation: -1.0, warmth: 9.0, night_cool: 9.0,
            cas_strength: 9.0, motion_blur: 9.0, dof_aperture: 9.0, dof_max_radius: 99.0, ..defaults }.clamped();
        assert_eq!((bounded.vignette, bounded.contrast, bounded.saturation, bounded.warmth, bounded.night_cool), (1.0, 2.0, 0.0, 1.0, 1.0));
        assert_eq!((bounded.cas_strength, bounded.motion_blur, bounded.dof_aperture, bounded.dof_max_radius), (1.0, 1.0, 8.0, 16.0));
    }

    #[test]
    fn foliage_switches_are_independent_and_default_off() {
        let defaults = EnhancedSettings::default();
        assert!(!defaults.foliage_enabled() && !defaults.interaction_enabled());
        macro_rules! check {
            ($($field:ident => $interaction:expr),+ $(,)?) => {$(
                assert!(!defaults.$field);
                let settings = EnhancedSettings { $field: true, ..defaults };
                assert!(settings.foliage_enabled() && settings.any_enabled());
                assert_eq!(settings.interaction_enabled(), $interaction);
                assert_eq!(settings.clamped(), settings);
            )+};
        }
        check!(
            foliage_edges => false, foliage_sway => false, foliage_push => true,
            foliage_leaves => false, foliage_flutter => false, foliage_falling => false,
            grass_trails => true, foliage_subdivision => true,
            foliage_translucency => false, foliage_lod => false,
        );
        assert!(EnhancedSettings { ground_grass: true, ..defaults }.interaction_enabled());
    }

    #[test]
    fn defaults_flags_and_quality_bounds() {
        let defaults = EnhancedSettings::default();
        assert!(!defaults.any_enabled());
        assert_eq!(defaults.bloom_strength, 0.6);
        assert_eq!(defaults.clamped(), defaults);
        assert_eq!(defaults.point_shadow_count, 4);
        for (input, expected) in [(0, 1), (1, 1), (4, 4), (16, 16), (u32::MAX, 16)] {
            assert_eq!(EnhancedSettings { point_shadow_count: input, ..defaults }.clamped().point_shadow_count, expected);
        }
        for mask in 0..(1 << 18) {
            let settings = EnhancedSettings {
                sun_shadows: mask & 1 != 0, ambient_occlusion: mask & 2 != 0,
                emissive_bloom: mask & 4 != 0, ground_grass: mask & 8 != 0, hdr: mask & 16 != 0, contact_shadows: mask & 32 != 0,
                height_haze: mask & 64 != 0, land_haze: mask & 128 != 0, valley_mist: mask & 256 != 0,
                volumetric_shafts: mask & 512 != 0, cloud_gap_shafts: mask & 1024 != 0,
                volumetric_clouds: mask & 2048 != 0, cloud_shadows: mask & 4096 != 0, sky_discs: mask & 8192 != 0,
                moon_shadows: mask & 16384 != 0, material_specular: mask & 32768 != 0,
                clustered_lighting: mask & 65536 != 0, local_light_shadows: mask & 131072 != 0,
                ao_strength: 0.0, bloom_strength: 0.0, grass_density: 0.0, sun_boost: 0.0, grade_strength: 0.0, contact_strength: 0.0, ..defaults
            };
            assert_eq!(settings.any_enabled(), mask != 0);
            assert_eq!(settings.needs_sun_mask(), mask & (16 | 32 | 64 | 128 | 256 | 512 | 1024 | 4096 | 8192) != 0);
            assert_eq!(settings.needs_admitted_lights(), mask & (32768 | 65536 | 131072) != 0);
        }
        for mask in 0..1024 {
            let settings = EnhancedSettings {
                water_waves: mask & 1 != 0, water_ripples: mask & 2 != 0,
                water_refraction: mask & 4 != 0, water_reflections: mask & 8 != 0,
                water_foam: mask & 16 != 0, water_wakes: mask & 32 != 0,
                waterfalls: mask & 64 != 0, underwater: mask & 128 != 0,
                water_caustics: mask & 256 != 0, water_sun_glint: mask & 512 != 0, ..defaults
            };
            assert_eq!(settings.any_enabled(), mask != 0);
            assert_eq!(settings.clamped(), settings);
        }
        assert_eq!(defaults.water_realism, 0.5);
        assert_eq!(EnhancedSettings { water_realism: -1.0, ..defaults }.clamped().water_realism, 0.0);
        assert_eq!(EnhancedSettings { water_realism: 2.0, ..defaults }.clamped().water_realism, 1.0);
        for value in [-1.0, 10.0] {
            let settings = EnhancedSettings {
                water_wake_strength: value, water_wake_visibility: value,
                water_wake_duration: value, water_wake_speed: value,
                water_wake_treading: value, water_wake_splash: value, ..defaults
            }.clamped();
            assert_eq!([settings.water_wake_strength, settings.water_wake_visibility,
                settings.water_wake_duration, settings.water_wake_speed,
                settings.water_wake_treading, settings.water_wake_splash],
                if value < 0.0 { [0.0, 0.0, 0.05, 0.1, 0.0, 0.0] } else { [4.0, 4.0, 2.0, 8.0, 3.0, 3.0] });
            assert!(!settings.any_enabled());
        }
        assert_eq!(EnhancedSettings { sun_boost: 9.0, ..defaults }.clamped().sun_boost, 0.6);
        for night_mode in [NightMode::Moonlit, NightMode::Dark] {
            assert!(EnhancedSettings {night_mode, ..defaults}.any_enabled());
        }
        let night = EnhancedSettings {moon_intensity: 9.0, dark_night_strength: 2.0, ..defaults}.clamped();
        assert_eq!((night.moon_intensity, night.dark_night_strength), (4.0, 1.0));
        assert_eq!(EnhancedSettings { contact_strength: 9.0, ..defaults }.clamped().contact_strength, 1.0);
        assert!(EnhancedSettings { contact_shadows: true, ..defaults }.needs_sun_mask() && !defaults.needs_sun_mask());
        let bounded = EnhancedSettings {
            shadow_map_size: 0, ao_radius: -1.0, ao_strength: 3.0, bloom_strength: 2.0,
            grass_density: 4.0, grass_distance: 200.0, ..defaults
        }.clamped();
        assert_eq!((bounded.shadow_map_size, bounded.ao_radius, bounded.ao_strength), (256, 0.0, 2.0));
        assert_eq!((bounded.bloom_strength, bounded.grass_density, bounded.grass_distance), (1.0, 2.0, 180.0));
        assert_eq!(defaults.shadow_map_size, 2048);
        let shadow = EnhancedSettings { shadow_strength: 2.0, shadow_softness: 9.0, ..defaults }.clamped();
        assert_eq!((shadow.shadow_strength, shadow.shadow_softness), (1.0, 4.0));
        assert_eq!(EnhancedSettings { shadow_map_size: u32::MAX, ..defaults }.clamped().shadow_map_size, 4096);
        for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(EnhancedSettings { water_realism: invalid, ..defaults }.clamped(), defaults);
            assert_eq!(EnhancedSettings {
                water_wake_strength: invalid, water_wake_visibility: invalid,
                water_wake_duration: invalid, water_wake_speed: invalid,
                water_wake_treading: invalid, water_wake_splash: invalid, ..defaults
            }.clamped(), defaults);
            assert_eq!(EnhancedSettings {
                shadow_strength: invalid, shadow_softness: invalid, ao_radius: invalid, ao_strength: invalid, bloom_strength: invalid,
                moon_intensity: invalid, dark_night_strength: invalid,
                lamp_intensity: invalid, carried_light_intensity: invalid,
                grass_density: invalid, grass_distance: invalid, sun_boost: invalid, grade_strength: invalid, contact_strength: invalid, ..defaults
            }.clamped(), defaults);
        }
    }
}
