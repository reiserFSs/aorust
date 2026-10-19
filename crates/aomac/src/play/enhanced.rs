//! Native Enhanced preferences: one inventory for layout, registration and reset.

use super::dvalue::{DValues, Kind, Variant};
use super::options::model::{Ctl, Item, Opt, Page, Ty};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GraphicsPreset { Original, ClassicPlus, Modern, Ultra, EnhancedV2High = 5 }

impl GraphicsPreset {
    pub const ALL: [Self; 5] = [Self::Original, Self::ClassicPlus, Self::Modern, Self::Ultra, Self::EnhancedV2High];

    #[cfg(test)]
    pub fn name(self) -> &'static str {
        match self { Self::Original => "Original", Self::ClassicPlus => "Classic+", Self::Modern => "Modern", Self::Ultra => "Ultra", Self::EnhancedV2High => "Enhanced v2 High" }
    }

    #[cfg(test)]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|preset| preset.name() == name)
    }

    pub fn msaa(self) -> u32 { if self == Self::ClassicPlus { 4 } else { 1 } }
    pub fn anisotropy(self) -> u16 {
        match self { Self::Original => 1, Self::ClassicPlus => 8, Self::Modern | Self::Ultra | Self::EnhancedV2High => 16 }
    }

    pub fn index(self) -> i64 { self as i64 }
}

/// UI, camera and diagnostic preferences are independent of graphical presets.
pub(super) fn graphical(name: &str) -> bool {
    name.starts_with("Aomac") && !matches!(name,
        "AomacGraphicsPreset" | "AomacEnhancedSuspended" | "AomacEffectGpuOverlay" | "AomacSlowFrameLog"
        | "AomacInterfaceScale" | "AomacBorderlessFullscreen" | "AomacPinchZoom"
        | "AomacWidescreenFOV" | "AomacSmoothText")
}

/// Apply the entire graphical inventory, including inactive quality values, deterministically.
pub(super) fn apply_preset(d: &mut DValues, preset: GraphicsPreset) {
    for control in controls().into_iter().filter(|control| graphical(control.name)) {
        let value = if matches!(preset, GraphicsPreset::Modern | GraphicsPreset::Ultra)
            && matches!(control.kind, ControlKind::Check) { Variant::Bool(true) } else { control.default };
        d.set_raw(control.name, value);
    }
    // Enhanced v2 owns its look: every v1 switch stays at its default (off).
    if matches!(preset, GraphicsPreset::ClassicPlus | GraphicsPreset::Modern | GraphicsPreset::Ultra) {
        for name in ["AomacSunShadows", "AomacAmbientOcclusion", "AomacEmissiveBloom",
            "AomacHdr", "AomacGradingEnabled", "AomacContactShadows",
            "AomacClusteredLighting", "AomacMaterialSpecular", "AomacLocalLightShadows"] {
            d.set_raw(name, Variant::Bool(true));
        }
    }
    if preset == GraphicsPreset::ClassicPlus {
        for (name, value) in [("AomacSunBoost", 0.15), ("AomacGrade", 0.25),
            ("AomacBloomStrength", 0.2), ("AomacAoStrength", 0.6), ("AomacContactStrength", 0.4)] {
            d.set_raw(name, Variant::Float(value));
        }
    }
    if matches!(preset, GraphicsPreset::Modern | GraphicsPreset::Ultra) {
        d.set_raw("AomacNightMode", Variant::Int(1));
        d.set_raw("AomacNightCool", Variant::Float(0.15));
        d.set_raw("AomacVignette", Variant::Float(0.1));
        d.set_raw("AomacSceneryDistance", Variant::Float(1.0));
        // TAA + CAS is the recommended temporal path; cinematic/user-asset effects stay opt-in.
        for name in ["AomacFxaa", "AomacDepthOfField", "AomacZoneLut", "AomacIndirectLight"] {
            d.set_raw(name, Variant::Bool(false));
        }
        d.set_raw("AomacMotionBlur", Variant::Float(0.0));
    }
    if preset == GraphicsPreset::Modern {
        for (name, value) in [("AomacShadowMapSize", 1024), ("AomacShadowQuality", 0),
            ("AomacAoQuality", 0), ("AomacPointShadowCount", 2)] {
            d.set_raw(name, Variant::Int(value));
        }
        d.set_raw("AomacGrassDensity", Variant::Float(0.5));
        d.set_raw("AomacGrassDistance", Variant::Float(96.0));
    }
    if preset == GraphicsPreset::Ultra {
        for (name, value) in [("AomacShadowMapSize", 2048), ("AomacShadowQuality", 1),
            ("AomacPointShadowCount", 4), ("AomacCloudQuality", 2), ("AomacWaterQuality", 2), ("AomacAoQuality", 1)] {
            d.set_raw(name, Variant::Int(value));
        }
        for (name, value) in [("AomacAoRadius", 2.5), ("AomacAoStrength", 1.8),
            ("AomacGrassDensity", 1.0), ("AomacGrassDistance", 144.0)] {
            d.set_raw(name, Variant::Float(value));
        }
    }
    d.set_raw("AomacTextureSharpening", Variant::Int(match preset.anisotropy() { 1 => 0, 8 => 3, _ => 4 }));
    d.set_raw("AomacEdgeSmoothing", Variant::Int(if preset.msaa() == 1 { 0 } else { 2 }));
    d.set_raw("AomacGraphicsPreset", Variant::Int(preset.index()));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PageId {
    Presets,
    Lighting,
    Ambient,
    Sky,
    Water,
    Foliage,
    Camera,
    Performance,
}

impl PageId {
    pub const ALL: [Self; 8] = [Self::Presets, Self::Lighting, Self::Ambient, Self::Sky, Self::Water, Self::Foliage, Self::Camera, Self::Performance];

    pub fn label(self) -> &'static str {
        match self {
            Self::Presets => "Presets",
            Self::Lighting => "Lighting & Shadows",
            Self::Ambient => "Ambient & Bloom",
            Self::Sky => "Sky & Weather",
            Self::Water => "Water",
            Self::Foliage => "Grass & Plants",
            Self::Camera => "Camera & Post",
            Self::Performance => "Performance",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum ControlKind {
    Check,
    Slider { format: &'static str, scale: f64 },
    Radio(&'static [(&'static str, i32)]),
}

pub(super) struct Control {
    pub page: PageId,
    pub section: &'static str,
    pub name: &'static str,
    pub label: &'static str,
    pub kind: ControlKind,
    pub default: Variant,
    pub min: Option<Variant>,
    pub max: Option<Variant>,
}

impl Control {
    pub fn session_only(&self) -> bool {
        matches!(self.name, "AomacEnhancedSuspended" | "AomacEffectGpuOverlay" | "AomacSlowFrameLog")
    }

    fn check(page: PageId, section: &'static str, name: &'static str, label: &'static str) -> Self {
        Self { page, section, name, label, kind: ControlKind::Check, default: Variant::Bool(false), min: None, max: None }
    }

    #[allow(clippy::too_many_arguments)]
    fn slider(page: PageId, section: &'static str, name: &'static str, label: &'static str, format: &'static str, default: f32, min: f32, max: f32) -> Self {
        Self { page, section, name, label, kind: ControlKind::Slider { format, scale: 1.0 }, default: Variant::Float(default), min: Some(Variant::Float(min)), max: Some(Variant::Float(max)) }
    }

    #[allow(clippy::too_many_arguments)]
    fn radio(page: PageId, section: &'static str, name: &'static str, label: &'static str, choices: &'static [(&'static str, i32)], default: i64, min: i64, max: i64) -> Self {
        Self { page, section, name, label, kind: ControlKind::Radio(choices), default: Variant::Int(default), min: Some(Variant::Int(min)), max: Some(Variant::Int(max)) }
    }

    fn opt(&self) -> Opt {
        let (ctl, fmt, scale) = match self.kind {
            ControlKind::Check => (Ctl::Check, "", 1.0),
            ControlKind::Slider { format, scale } => (Ctl::Slider, format, scale),
            ControlKind::Radio(choices) => (Ctl::Radio(choices.iter().map(|&(label, value)| (label.to_string(), value)).collect()), "", 1.0),
        };
        Opt { ctl, var: self.name.to_string(), ty: Ty::Variant, kind: Kind::Login, label: self.label.to_string(), tooltip: None, enable: String::new(), bit: None, fmt: fmt.to_string(), scale, borders: "Rect(0,0,0,5)".into() }
    }
}

/// Retail ChatFontSize remains registered by MainPrefs.xml, not by this inventory.
pub(super) fn controls() -> Vec<Control> {
    use PageId::*;
    vec![
        Control::radio(Presets, "Graphics Preset", "AomacGraphicsPreset", "Graphics Preset",
            &[("Original", 0), ("Classic+", 1), ("Modern", 2), ("Ultra", 3), ("Enhanced v2 High", 5), ("Custom", 4)], 0, 0, 5),
        Control::check(Lighting, "Sun Shadows", "AomacSunShadows", "Sun Shadows"),
        Control::radio(Lighting, "Sun Shadows", "AomacShadowMapSize", "Shadow Map Size", &[("256", 256), ("512", 512), ("1024", 1024), ("2048", 2048), ("4096", 4096)], 2048, 256, 4096),
        Control::radio(Lighting, "Sun Shadows", "AomacShadowQuality", "Shadow Receiver Quality", &[("Low", 0), ("Normal", 1)], 1, 0, 1),
        Control::slider(Lighting, "Sun Shadows", "AomacShadowStrength", "Sun Shadow Strength", "%.2f", 0.45, 0.0, 1.0),
        Control::slider(Lighting, "Sun Shadows", "AomacShadowSoftness", "Sun Shadow Softness", "%.2f", 1.5, 0.0, 4.0),
        Control::slider(Lighting, "Sunlight", "AomacSunBoost", "Sunlight Brightness", "%.2f", 0.6, 0.0, 0.6),
        Control::check(Lighting, "Contact Shadows", "AomacContactShadows", "Contact Shadows"),
        Control::slider(Lighting, "Contact Shadows", "AomacContactStrength", "Contact Shadow Strength", "%.2f", 0.8, 0.0, 1.0),
        Control::check(Lighting, "Indirect Light", "AomacIndirectLight", "Screen-space Indirect Light"),
        Control::slider(Lighting, "Indirect Light", "AomacIndirectStrength", "Diffuse Bounce Strength", "%.2f", 1.0, 0.0, 3.0),
        Control::check(Lighting, "Moon and Night", "AomacMoonShadows", "Moon Shadows"),
        Control::radio(Lighting, "Moon and Night", "AomacNightMode", "Night Lighting", &[("As Game", 0), ("Moonlit", 1), ("Dark", 2)], 0, 0, 2),
        Control::slider(Lighting, "Moon and Night", "AomacMoonIntensity", "Moon Intensity", "%.2fx", 1.0, 0.0, 4.0),
        Control::slider(Lighting, "Moon and Night", "AomacDarkNightStrength", "Dark Night Strength", "%.2f", 0.6, 0.0, 1.0),
        Control::check(Lighting, "Local Lights", "AomacClusteredLighting", "Clustered Local Lighting"),
        Control::check(Lighting, "Local Lights", "AomacLocalLightShadows", "Local Light Shadows"),
        Control { page: Lighting, section: "Local Lights", name: "AomacPointShadowCount", label: "Point Shadow Count", kind: ControlKind::Slider { format: "%.0f", scale: 1.0 }, default: Variant::Int(4), min: Some(Variant::Int(1)), max: Some(Variant::Int(16)) },
        Control::slider(Lighting, "Local Lights", "AomacLampIntensity", "Lamp Intensity", "%.2fx", 1.0, 0.0, 4.0),
        Control::slider(Lighting, "Local Lights", "AomacCarriedLightIntensity", "Carried Light Intensity", "%.2fx", 1.0, 0.0, 4.0),
        Control::check(Lighting, "Material Highlights", "AomacMaterialSpecular", "Authored Material Specular (GGX + AA)"),
        Control::check(Lighting, "Characters", "AomacCharacterSmoothing", "Round Character Silhouettes (nearby)"),
        Control::check(Lighting, "Materials & Reflections", "AomacGeneratedNormals", "Generated Texture Bump Maps"),
        Control::check(Lighting, "Materials & Reflections", "AomacMaterialMaps", "Side-loaded Material Maps"),
        Control::check(Lighting, "Materials & Reflections", "AomacMaterialPbr", "Texture Material GGX"),
        Control::check(Lighting, "Materials & Reflections", "AomacMaterialReflections", "Screen-space Material Reflections"),
        Control::check(Lighting, "Materials & Reflections", "AomacEnvironmentProbe", "Observed Static Environment Probe"),
        Control::check(Ambient, "Ambient Occlusion", "AomacAmbientOcclusion", "Ambient Occlusion"),
        Control::radio(Ambient, "Ambient Occlusion", "AomacAoQuality", "Ambient Occlusion Quality", &[("Low", 0), ("Normal", 1), ("High", 2)], 1, 0, 2),
        Control::slider(Ambient, "Ambient Occlusion", "AomacAoRadius", "Ambient Occlusion Radius", "%.2f m", 1.75, 0.0, 5.0),
        Control::slider(Ambient, "Ambient Occlusion", "AomacAoStrength", "Ambient Occlusion Strength", "%.2f", 1.5, 0.0, 2.0),
        Control::check(Ambient, "Emissive Bloom", "AomacEmissiveBloom", "Emissive Bloom"),
        Control::slider(Ambient, "Emissive Bloom", "AomacBloomStrength", "Emissive Bloom Strength", "%.2f", 0.6, 0.0, 1.0),
        Control::check(Sky, "Haze & Mist", "AomacHeightHaze", "Height Haze"),
        Control::check(Sky, "Haze & Mist", "AomacLandHaze", "Land Haze"),
        Control::check(Sky, "Haze & Mist", "AomacValleyMist", "Valley Mist"),
        Control::check(Sky, "Light Shafts", "AomacVolumetricShafts", "Volumetric Shafts"),
        Control::check(Sky, "Light Shafts", "AomacCloudGapShafts", "Cloud-gap Shafts"),
        Control::check(Sky, "Clouds & Sky", "AomacVolumetricClouds", "Volumetric Clouds"),
        Control::check(Sky, "Clouds & Sky", "AomacCloudShadows", "Cloud Shadows"),
        Control::check(Sky, "Clouds & Sky", "AomacSkyDiscs", "Sky Discs"),
        Control::radio(Sky, "Clouds & Sky", "AomacCloudQuality", "Cloud Quality", &[("Normal", 1), ("High", 2)], 1, 1, 2),
        Control::check(Water, "Surface", "AomacWaterWaves", "Water Waves"),
        Control::check(Water, "Surface", "AomacWaterRipples", "Water Ripples"),
        Control::check(Water, "Surface", "AomacWaterFoam", "Water Foam"),
        Control::check(Water, "Optics", "AomacWaterRefraction", "Water Refraction"),
        Control::check(Water, "Optics", "AomacWaterReflections", "Water Reflections"),
        Control::check(Water, "Optics", "AomacWaterCaustics", "Water Caustics"),
        Control::check(Water, "Optics", "AomacWaterSunGlint", "Water Sun Glint"),
        Control::radio(Water, "Optics", "AomacWaterQuality", "Water Quality", &[("Normal", 1), ("High", 2)], 1, 1, 2),
        Control::slider(Water, "Optics", "AomacWaterRealism", "Water Realism (native colour to realistic)", "%.2f", 0.5, 0.0, 1.0),
        Control::check(Water, "Wakes", "AomacWaterWakes", "Water Wakes"),
        Control::slider(Water, "Wakes", "AomacWaterWakeStrength", "Water Wake Strength", "%.2f", 2.0, 0.0, 4.0),
        Control::slider(Water, "Wakes", "AomacWaterWakeVisibility", "Water Wake Visibility", "%.2f", 2.0, 0.0, 4.0),
        Control::slider(Water, "Wakes", "AomacWaterWakeDuration", "Water Wake Duration", "%.2f", 0.3, 0.05, 2.0),
        Control::slider(Water, "Wakes", "AomacWaterWakeSpeed", "Water Wake Speed", "%.2f", 3.6, 0.1, 8.0),
        Control::slider(Water, "Wakes", "AomacWaterWakeTreading", "Water Wake Treading", "%.2f", 1.0, 0.0, 3.0),
        Control::slider(Water, "Wakes", "AomacWaterWakeSplash", "Water Wake Splash", "%.2f", 1.0, 0.0, 3.0),
        Control::check(Water, "Waterfalls & Underwater", "AomacWaterfalls", "Waterfalls"),
        Control::check(Water, "Waterfalls & Underwater", "AomacUnderwater", "Underwater"),
        Control::check(Sky, "Rain & Surfaces", "AomacEnhancedRain", "Enhanced Rain"),
        Control::check(Sky, "Rain & Surfaces", "AomacWetSurfaces", "Wet Surfaces"),
        Control::check(Sky, "Weather & Effects", "AomacWeatherParticles", "Storm Particles"),
        Control::check(Sky, "Weather & Effects", "AomacGpuSpellParticles", "GPU Spell Particles"),
        Control::check(Sky, "Weather & Effects", "AomacFluidSmoke", "Fluid Smoke"),
        Control::check(Foliage, "Ground Grass", "AomacGroundGrass", "Ground Grass"),
        Control::check(Foliage, "Ground Grass", "AomacGrassTufts", "Grass Tufts"),
        Control::check(Foliage, "Ground Grass", "AomacGrassSpecies", "Grass Species"),
        Control::check(Foliage, "Ground Grass", "AomacGrassPathThinning", "Grass Path Thinning"),
        Control::check(Foliage, "Ground Grass", "AomacGrassGustBands", "Grass Gust Bands"),
        Control::check(Foliage, "Ground Grass", "AomacGrassSunlight", "Grass Sunlight"),
        Control::check(Foliage, "Ground Grass", "AomacGrassSunShadows", "Grass Sun Shadows"),
        Control::check(Foliage, "Ground Grass", "AomacGrassPointShadows", "Grass Point Shadows"),
        Control::slider(Foliage, "Ground Grass", "AomacGrassDensity", "Ground Grass Density", "%.2fx", 1.0, 0.0, 2.0),
        Control::slider(Foliage, "Ground Grass", "AomacGrassDistance", "Ground Grass Distance", "%.1f m", 160.0, 0.0, 180.0),
        Control::check(Foliage, "Native Foliage", "AomacFoliageEdges", "Foliage Edges"),
        Control::check(Foliage, "Native Foliage", "AomacFoliageSway", "Foliage Sway"),
        Control::check(Foliage, "Native Foliage", "AomacFoliagePush", "Foliage Push"),
        Control::check(Foliage, "Native Foliage", "AomacFoliageLeaves", "Canopy Leaves"),
        Control::check(Foliage, "Native Foliage", "AomacFoliageFlutter", "Leaf Flutter"),
        Control::check(Foliage, "Native Foliage", "AomacFoliageFalling", "Falling Leaves"),
        Control::check(Foliage, "Native Foliage", "AomacGrassTrails", "Grass Trails"),
        Control::check(Foliage, "Native Foliage", "AomacFoliageSubdivision", "Foliage Subdivision"),
        Control::check(Foliage, "Native Foliage", "AomacFoliageTranslucency", "Foliage Translucency"),
        Control::check(Foliage, "Native Foliage", "AomacFoliageLod", "Foliage LOD"),
        Control::check(Camera, "Camera", "AomacPinchZoom", "Pinch to Zoom (trackpad)"),
        Control::check(Camera, "Camera", "AomacWidescreenFOV", "Widescreen FOV (keep the 4:3 vertical angle)"),
        Control::check(Camera, "Post Processing", "AomacHdr", "HDR Sunlight and Tonemap"),
        Control::slider(Camera, "Post Processing", "AomacGrade", "Colour Grade (warm sun, cool shade)", "%.2f", 0.8, 0.0, 1.0),
        Control::check(Camera, "Colour Grading", "AomacGradingEnabled", "Warm Sun / Cool Shade Grade"),
        Control::slider(Camera, "Colour Grading", "AomacContrast", "Contrast", "%.2f", 1.0, 0.0, 2.0),
        Control::slider(Camera, "Colour Grading", "AomacSaturation", "Saturation", "%.2f", 1.0, 0.0, 2.0),
        Control::slider(Camera, "Colour Grading", "AomacWarmth", "Warmth (0.50 neutral)", "%.2f", 0.5, 0.0, 1.0),
        Control::slider(Camera, "Colour Grading", "AomacNightCool", "Cooler Night Tint", "%.2f", 0.0, 0.0, 1.0),
        Control::slider(Camera, "Colour Grading", "AomacVignette", "Vignette", "%.2f", 0.0, 0.0, 1.0),
        Control::check(Camera, "Colour Grading", "AomacZoneLut", "Day / Night and Per-zone User .cube LUTs"),
        Control::check(Camera, "Antialiasing", "AomacTaa", "Temporal Antialiasing"),
        Control::check(Camera, "Antialiasing", "AomacFxaa", "FXAA (when TAA is off)"),
        Control::check(Camera, "Sharpening", "AomacCas", "Contrast Adaptive Sharpening"),
        Control::slider(Camera, "Sharpening", "AomacCasStrength", "Sharpening Strength", "%.2f", 0.5, 0.0, 1.0),
        Control::slider(Camera, "Motion Blur", "AomacMotionBlur", "Motion Blur", "%.2f", 0.0, 0.0, 1.0),
        Control::check(Camera, "Depth of Field", "AomacDepthOfField", "Depth of Field"),
        Control::slider(Camera, "Depth of Field", "AomacDofAperture", "Depth of Field Aperture", "%.2f", 2.0, 0.0, 8.0),
        Control::slider(Camera, "Depth of Field", "AomacDofMaxRadius", "Depth of Field Max Radius", "%.2f px", 8.0, 0.0, 16.0),
        Control::check(Performance, "Session Controls", "AomacEnhancedSuspended", "Disable All Enhanced Effects (Ctrl+Shift+E, session only)"),
        Control::check(Performance, "Session Controls", "AomacEffectGpuOverlay", "Effect GPU Costs (Ctrl+Shift+O, session only)"),
        Control::check(Performance, "Session Controls", "AomacSlowFrameLog", "Log Slow Frames (session only)"),
        Control::radio(Performance, "Interface", "AomacInterfaceScale", "Interface Scale", &[("Auto (retail-equivalent)", 0), ("1x", 1), ("2x", 2), ("3x", 3)], 0, 0, 3),
        Control::check(Performance, "Interface", "AomacBorderlessFullscreen", "Instant Fullscreen (skip animation, hide menu bar/dock)"),
        Control { page: Performance, section: "Text", name: "ChatFontSize", label: "Chat Font Size", kind: ControlKind::Slider { format: "%.0f pt", scale: 0.1 }, default: Variant::Int(140), min: Some(Variant::Int(50)), max: Some(Variant::Int(480)) },
        Control::check(Performance, "Text", "AomacSmoothText", "Anti-aliased Text (Retina, TrueType fonts only)"),
        Control::radio(Performance, "Texture Filtering", "AomacTextureSharpening", "Texture Sharpening (anisotropic filtering)", &[("Off (1x, retail)", 0), ("2x", 1), ("4x", 2), ("8x", 3), ("16x", 4)], 0, 0, 4),
        Control::check(Performance, "Texture Filtering", "AomacBetterMipFiltering", "Better Mip Filtering (gamma-correct, coverage-preserving foliage)"),
        Control::radio(Performance, "Edge Smoothing", "AomacEdgeSmoothing", "Edge Smoothing (MSAA)", &[("Off (retail)", 0), ("2x", 1), ("4x", 2), ("8x", 3)], 0, 0, 3),
        Control::slider(Performance, "Scenery", "AomacSceneryDistance", "Extended Scenery Distance (terrain/statels only, 1.0x = retail)", "%.2fx", 1.0, 1.0, 2.0),
    ]
}

pub(super) fn pages() -> Vec<Page> {
    let controls = controls();
    PageId::ALL.into_iter().map(|page| {
        let mut items = Vec::new();
        if page == PageId::Presets {
            items.push(Item::Text { value: "Original: faithful graphics. Classic+: mild lighting, no new scenery.".into(), borders: "Rect(0,5,0,5)".into() });
            items.push(Item::Text { value: "Modern: recommended effects. Ultra: same effects, higher scene quality.".into(), borders: "Rect(0,0,0,5)".into() });
            items.push(Item::Text { value: "Manual graphical edits select Custom, even when changed back.".into(), borders: "Rect(0,0,0,10)".into() });
        }
        let mut section = None;
        for control in controls.iter().filter(|control| control.page == page) {
            if section != Some(control.section) {
                if section.is_some() {
                    items.push(Item::Spacer { min: 10.0, max: 10.0 });
                }
                items.push(Item::Text { value: control.section.to_string(), borders: "Rect(0,5,0,5)".into() });
                section = Some(control.section);
            }
            items.push(Item::Opt(control.opt()));
        }
        if section.is_none() && page != PageId::Presets {
            items.push(Item::Text { value: format!("No Enhanced {} controls are registered.", page.label().to_ascii_lowercase()), borders: "Rect(0,5,0,10)".into() });
        }
        Page { label: format!("Enhanced/{}", page.label()), items }
    }).collect()
}

/// Reset only registered Enhanced controls; unrelated retail preferences are untouched.
pub(super) fn reset(d: &mut DValues, page: Option<PageId>) {
    if page == Some(PageId::Presets) {
        apply_preset(d, GraphicsPreset::Original);
        return;
    }
    for control in controls().into_iter().filter(|control| page.is_none_or(|page| page == control.page)) {
        d.set_raw(control.name, control.default);
    }
    if page.is_none() {
        d.set_raw("AomacGraphicsPreset", Variant::Int(0));
    } else if page != Some(PageId::Presets) {
        d.set_raw("AomacGraphicsPreset", Variant::Int(4));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::dvalue::CAT_MAIN;
    use std::collections::BTreeSet;

    fn values() -> DValues {
        let mut d = DValues::new(std::path::Path::new("/nonexistent/aomac-enhanced-registry-test"));
        // Retail owns this registration; exercise it without requiring an installed client.
        d.load_config("<Root><Value name=\"ChatFontSize\" value=\"140\" min=\"50\" max=\"480\"/></Root>", CAT_MAIN, true);
        d
    }

    fn changed(control: &Control) -> Variant {
        match &control.default {
            Variant::Bool(value) => Variant::Bool(!value),
            _ => [control.min.as_ref(), control.max.as_ref()].into_iter().flatten().find(|value| **value != control.default).unwrap().clone(),
        }
    }

    #[test]
    fn ao_quality_is_a_bounded_independent_ambient_radio() {
        let mut d = values();
        let inventory = controls();
        let quality = inventory.iter().find(|control| control.name == "AomacAoQuality").unwrap();
        assert_eq!(quality.page, PageId::Ambient);
        assert!(matches!(quality.kind, ControlKind::Radio(choices) if choices == [("Low", 0), ("Normal", 1), ("High", 2)]));
        assert_eq!(d.min_max(quality.name), (Variant::Int(0), Variant::Int(2)));
        d.set(quality.name, Variant::Int(0));
        assert!(!d.enhanced_settings().any_enabled());
        d.set("AomacAmbientOcclusion", Variant::Bool(true));
        assert!(d.enhanced_settings().ambient_occlusion);
        assert_eq!(d.enhanced_settings().ao_quality, 0);
        reset(&mut d, Some(PageId::Ambient));
        assert_eq!(d.enhanced_settings(), ao_render::EnhancedSettings::default());
    }
    #[test]
    fn shadow_quality_is_bounded_persisted_and_independent() {
        let mut d = values();
        let inventory = controls();
        let quality = inventory.iter().find(|control| control.name == "AomacShadowQuality").unwrap();
        assert_eq!(quality.page, PageId::Lighting);
        assert!(matches!(quality.kind, ControlKind::Radio(choices) if choices == [("Low", 0), ("Normal", 1)]));
        assert_eq!(d.min_max(quality.name), (Variant::Int(0), Variant::Int(1)));
        assert_eq!(d.enhanced_settings().shadow_quality, 1);
        for (input, expected) in [(-1, 0), (0, 0), (1, 1), (2, 1), (i64::MAX, 1)] {
            d.set(quality.name, Variant::Int(input));
            let mut restored = values();
            restored.load_config(&d.save_config(CAT_MAIN), CAT_MAIN, false);
            assert_eq!(restored.enhanced_settings().shadow_quality, expected);
            assert!(!restored.enhanced_settings().any_enabled());
        }
        reset(&mut d, Some(PageId::Lighting));
        assert_eq!(d.enhanced_settings(), ao_render::EnhancedSettings::default());
    }


    #[test]
    fn indirect_light_maps_independently_and_resets_with_lighting() {
        let mut d = values();
        let defaults = ao_render::EnhancedSettings::default();
        assert_eq!(d.enhanced_settings(), defaults);
        let controls = controls();
        let toggle = controls.iter().find(|control| control.name == "AomacIndirectLight").unwrap();
        let strength = controls.iter().find(|control| control.name == "AomacIndirectStrength").unwrap();
        assert_eq!(toggle.page, PageId::Lighting);
        assert_eq!(toggle.label, "Screen-space Indirect Light");
        assert_eq!(strength.page, PageId::Lighting);
        assert_eq!(d.min_max(strength.name), (Variant::Float(0.0), Variant::Float(3.0)));
        assert!(d.set(strength.name, Variant::Float(2.5)));
        assert_eq!(d.enhanced_settings(), ao_render::EnhancedSettings { indirect_strength: 2.5, ..defaults });
        assert!(!d.enhanced_settings().any_enabled());
        assert!(d.set(toggle.name, Variant::Bool(true)));
        let expected = ao_render::EnhancedSettings { indirect_light: true, indirect_strength: 2.5, ..defaults };
        assert_eq!(d.enhanced_settings(), expected);
        let mut loaded = values();
        assert!(loaded.load_config(&d.save_config(CAT_MAIN), CAT_MAIN, false));
        assert_eq!(loaded.enhanced_settings(), expected);
        reset(&mut loaded, Some(PageId::Ambient));
        assert_eq!(loaded.enhanced_settings(), expected);
        reset(&mut loaded, Some(PageId::Lighting));
        assert_eq!(loaded.enhanced_settings(), defaults);
    }
    #[test]
    fn material_switches_are_independent_default_off_and_live() {
        let mut d = values();
        let flags = |settings: ao_render::EnhancedSettings| [
            settings.generated_normals, settings.material_maps, settings.material_pbr,
            settings.material_reflections, settings.environment_probe,
        ];
        assert_eq!(flags(d.enhanced_settings()), [false; 5]);
        for (index, name) in [
            "AomacGeneratedNormals", "AomacMaterialMaps", "AomacMaterialPbr",
            "AomacMaterialReflections", "AomacEnvironmentProbe",
        ].into_iter().enumerate() {
            assert!(d.set(name, Variant::Bool(true)));
            let mut expected = [false; 5];
            expected[index] = true;
            assert_eq!(flags(d.enhanced_settings()), expected);
            reset(&mut d, Some(PageId::Lighting));
            assert_eq!(flags(d.enhanced_settings()), [false; 5]);
        }
    }

    #[test]
    fn character_smoothing_is_default_off_persistent_and_resettable() {
        let mut d = values();
        assert!(!d.enhanced_settings().character_smoothing);
        assert!(d.set("AomacCharacterSmoothing", Variant::Bool(true)));
        assert!(d.enhanced_settings().character_smoothing);
        let mut loaded = values();
        assert!(loaded.load_config(&d.save_config(CAT_MAIN), CAT_MAIN, false));
        assert!(loaded.enhanced_settings().character_smoothing);
        reset(&mut loaded, Some(PageId::Lighting));
        assert!(!loaded.enhanced_settings().character_smoothing);
    }


    #[test]
    fn registry_is_unique_complete_resettable_and_persistent() {
        let controls = controls();
        let names: BTreeSet<_> = controls.iter().map(|control| control.name).collect();
        assert_eq!(names.len(), controls.len());
        let pages = pages();
        assert_eq!(pages.len(), PageId::ALL.len());
        let mut layout_names = BTreeSet::new();
        for (page, id) in pages.iter().zip(PageId::ALL) {
            assert_eq!(page.label, format!("Enhanced/{}", id.label()));
            for item in &page.items {
                if let Item::Opt(opt) = item {
                    assert!(layout_names.insert(opt.var.as_str()));
                    assert_eq!(controls.iter().find(|control| control.name == opt.var).unwrap().page, id);
                }
            }
        }
        assert_eq!(layout_names, names);
        let mut d = values();
        for control in &controls {
            assert_eq!(d.get(control.name), Some(&control.default));
            assert_eq!(d.min_max(control.name), (control.min.clone().unwrap_or(Variant::Void), control.max.clone().unwrap_or(Variant::Void)));
            assert!(d.set_raw(control.name, changed(control)));
        }
        let saved = d.save_config(CAT_MAIN);
        let mut loaded = values();
        assert!(loaded.load_config(&saved, CAT_MAIN, false));
        for control in &controls {
            let expected = if control.session_only() { control.default.clone() } else { changed(control) };
            assert_eq!(loaded.get(control.name), Some(&expected));
            assert_eq!(loaded.persists(control.name), !control.session_only());
            if control.session_only() { assert!(!saved.contains(control.name)); }
        }
        for page in PageId::ALL.into_iter().filter(|page| *page != PageId::Presets) {
            for control in &controls {
                loaded.set_raw(control.name, changed(control));
            }
            reset(&mut loaded, Some(page));
            for control in controls.iter().filter(|control| control.name != "AomacGraphicsPreset") {
                let expected = if control.page == page { control.default.clone() } else { changed(control) };
                assert_eq!(loaded.get(control.name), Some(&expected));
            }
        }
        loaded.add("UnrelatedRetailPreference", Variant::Int(7), true, CAT_MAIN, None, None, false);
        loaded.restore_original_graphics();
        for control in &controls {
            assert_eq!(loaded.get(control.name), Some(&control.default));
        }
        assert_eq!(loaded.get("UnrelatedRetailPreference"), Some(&Variant::Int(7)));
    }

    #[test]
    fn preset_antialiasing_uses_and_persists_each_presets_own_samples() {
        let mut d = values();
        for (preset, samples, edge_smoothing, temporal) in [
            (GraphicsPreset::Original, 1, 0, false),
            (GraphicsPreset::ClassicPlus, 4, 2, false),
            (GraphicsPreset::Modern, 1, 0, true),
            (GraphicsPreset::Ultra, 1, 0, true),
        ] {
            for name in ["AomacTaa", "AomacCas", "AomacFxaa"] {
                d.set_raw(name, Variant::Bool(!temporal));
            }
            d.set_raw("AomacEdgeSmoothing", Variant::Int(3));
            d.set("AomacGraphicsPreset", Variant::Int(preset.index()));
            assert_eq!(preset.msaa(), samples, "{preset:?}");
            assert_eq!(d.get_i64("AomacEdgeSmoothing"), Some(edge_smoothing), "{preset:?}");
            let mut loaded = values();
            loaded.load_config(&d.save_config(CAT_MAIN), CAT_MAIN, false);
            assert_eq!(loaded.get_i64("AomacEdgeSmoothing"), Some(edge_smoothing), "{preset:?}");
            assert_eq!(loaded.flag("AomacTaa"), temporal, "{preset:?}");
            assert_eq!(loaded.flag("AomacCas"), temporal, "{preset:?}");
            assert!(!loaded.flag("AomacFxaa"), "{preset:?}");
        }
    }

    #[test]
    fn presets_cover_every_graphical_control_and_manual_edits_stay_custom() {
        let mut d = values();
        for preset in GraphicsPreset::ALL {
            assert_eq!(GraphicsPreset::from_name(preset.name()), Some(preset));
            d.set("AomacGraphicsPreset", Variant::Int(preset.index()));
            let expected = d.enhanced_settings();
            let saved = d.save_config(CAT_MAIN);
            let mut loaded = values();
            loaded.load_config(&saved, CAT_MAIN, false);
            assert_eq!(loaded.get_i64("AomacGraphicsPreset"), Some(preset.index()));
            assert_eq!(loaded.enhanced_settings(), expected);
            for control in controls().into_iter().filter(|control| graphical(control.name)) {
                let original = d.get(control.name).unwrap().clone();
                d.set(control.name, changed(&control));
                assert_eq!(d.get_i64("AomacGraphicsPreset"), Some(4), "{}", control.name);
                d.set(control.name, original);
                assert_eq!(d.get_i64("AomacGraphicsPreset"), Some(4));
                apply_preset(&mut d, preset);
                assert_eq!(d.enhanced_settings(), expected);
            }
            for control in controls().into_iter().filter(|control|
                !graphical(control.name) && control.name != "AomacGraphicsPreset") {
                d.set(control.name, changed(&control));
                assert_eq!(d.get_i64("AomacGraphicsPreset"), Some(preset.index()), "{}", control.name);
            }
            assert_eq!(d.get_i64("AomacEdgeSmoothing"), Some(if preset.msaa() == 1 { 0 } else { 2 }));
        }
        assert_eq!(GraphicsPreset::from_name("unknown"), None);
        apply_preset(&mut d, GraphicsPreset::Original);
        assert!(!d.enhanced_settings().any_enabled());
        apply_preset(&mut d, GraphicsPreset::ClassicPlus);
        let classic = d.enhanced_settings();
        assert!(classic.hdr && classic.sun_shadows && classic.ambient_occlusion && classic.contact_shadows);
        assert!(classic.clustered_lighting && classic.material_specular && classic.local_light_shadows);
        assert!(!classic.ground_grass && !classic.volumetric_clouds && !classic.water_waves);
        apply_preset(&mut d, GraphicsPreset::Modern);
        assert!(d.flag("AomacTaa") && d.flag("AomacCas") && !d.flag("AomacFxaa"));
        assert!(!d.flag("AomacDepthOfField") && !d.flag("AomacZoneLut"));
        assert_eq!(d.get_f32("AomacMotionBlur"), Some(0.0));
        let modern = d.enhanced_settings();
        assert!(!modern.indirect_light, "indirect light requires a measured budget before preset inclusion");
        apply_preset(&mut d, GraphicsPreset::Ultra);
        let ultra = d.enhanced_settings();
        for control in controls().into_iter().filter(|control|
            graphical(control.name) && matches!(control.kind, ControlKind::Check)) {
            let expected = !matches!(control.name, "AomacFxaa" | "AomacDepthOfField" | "AomacZoneLut" | "AomacIndirectLight");
            assert_eq!(d.flag(control.name), expected, "{}", control.name);
        }
        assert_eq!(ultra.motion_blur, 0.0);
        assert!(!ultra.indirect_light, "Ultra also keeps indirect light opt-in");
        assert_eq!((modern.shadow_map_size, ultra.shadow_map_size), (1024, 2048));
        assert_eq!((classic.shadow_quality, modern.shadow_quality, ultra.shadow_quality), (1, 0, 1));
        assert_eq!((modern.point_shadow_count, ultra.point_shadow_count), (2, 4));
        assert!(ultra.ao_radius > modern.ao_radius && ultra.ao_strength > modern.ao_strength);
        assert_eq!((classic.ao_quality, modern.ao_quality, ultra.ao_quality), (1, 0, 1));
        assert_eq!((modern.grass_density, modern.grass_distance), (0.5, 96.0));
        assert_eq!((ultra.grass_density, ultra.grass_distance), (1.0, 144.0));
        assert_eq!(d.get_i64("AomacCloudQuality"), Some(2));
        assert_eq!(d.get_i64("AomacWaterQuality"), Some(2));
        assert_eq!((ultra.cloud_quality, ultra.water_quality), (2, 2));
        let mut quality = values();
        assert_eq!((quality.enhanced_settings().cloud_quality, quality.enhanced_settings().water_quality), (1, 1));
        for input in [0, 1, 2, 3] {
            for name in ["AomacCloudQuality", "AomacWaterQuality"] {
                quality.set(name, Variant::Int(input));
            }
            let expected = input.clamp(1, 2) as u32;
            let mut restored = values();
            restored.load_config(&quality.save_config(CAT_MAIN), CAT_MAIN, false);
            let settings = restored.enhanced_settings();
            assert_eq!((settings.cloud_quality, settings.water_quality), (expected, expected));
        }
        assert_eq!(values().enhanced_settings().ao_quality, 1);
        for (input, expected) in [(-1, 0), (0, 0), (1, 1), (2, 2), (3, 2), (i64::MAX, 2)] {
            quality.set("AomacAoQuality", Variant::Int(input));
            let mut restored = values();
            restored.load_config(&quality.save_config(CAT_MAIN), CAT_MAIN, false);
            assert_eq!(restored.enhanced_settings().ao_quality, expected);
        }
        let mut legacy = values();
        legacy.load_config("<Root><Value name=\"AomacWaterWaves\" value=\"true\"/></Root>", CAT_MAIN, false);
        assert!(legacy.flag("AomacWaterWaves"));
        assert_eq!(legacy.get_i64("AomacGraphicsPreset"), Some(4));
    }
}
