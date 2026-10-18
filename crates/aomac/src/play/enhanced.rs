//! Native Enhanced preferences: one inventory for layout, registration and reset.

use super::dvalue::{DValues, Kind, Variant};
use super::options::model::{Ctl, Item, Opt, Page, Ty};

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
        Control::check(Lighting, "Sun Shadows", "AomacSunShadows", "Sun Shadows"),
        Control::radio(Lighting, "Sun Shadows", "AomacShadowMapSize", "Shadow Map Size", &[("256", 256), ("512", 512), ("1024", 1024), ("2048", 2048), ("4096", 4096)], 2048, 256, 4096),
        Control::slider(Lighting, "Sun Shadows", "AomacShadowStrength", "Sun Shadow Strength", "%.2f", 0.45, 0.0, 1.0),
        Control::slider(Lighting, "Sun Shadows", "AomacShadowSoftness", "Sun Shadow Softness", "%.2f", 1.5, 0.0, 4.0),
        Control::slider(Lighting, "Sunlight", "AomacSunBoost", "Sunlight Brightness", "%.2f", 0.6, 0.0, 0.6),
        Control::check(Lighting, "Contact Shadows", "AomacContactShadows", "Contact Shadows"),
        Control::slider(Lighting, "Contact Shadows", "AomacContactStrength", "Contact Shadow Strength", "%.2f", 0.8, 0.0, 1.0),
        Control::check(Lighting, "Moon and Night", "AomacMoonShadows", "Moon Shadows"),
        Control::radio(Lighting, "Moon and Night", "AomacNightMode", "Night Lighting", &[("As Game", 0), ("Moonlit", 1), ("Dark", 2)], 0, 0, 2),
        Control::slider(Lighting, "Moon and Night", "AomacMoonIntensity", "Moon Intensity", "%.2fx", 1.0, 0.0, 4.0),
        Control::slider(Lighting, "Moon and Night", "AomacDarkNightStrength", "Dark Night Strength", "%.2f", 0.6, 0.0, 1.0),
        Control::check(Ambient, "Ambient Occlusion", "AomacAmbientOcclusion", "Ambient Occlusion"),
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
        Control::check(Foliage, "Ground Grass", "AomacGroundGrass", "Ground Grass"),
        Control::slider(Foliage, "Ground Grass", "AomacGrassDensity", "Ground Grass Density", "%.2fx", 1.0, 0.0, 2.0),
        Control::slider(Foliage, "Ground Grass", "AomacGrassDistance", "Ground Grass Distance", "%.1f m", 160.0, 0.0, 180.0),
        Control::check(Camera, "Camera", "AomacPinchZoom", "Pinch to Zoom (trackpad)"),
        Control::check(Camera, "Camera", "AomacWidescreenFOV", "Widescreen FOV (keep the 4:3 vertical angle)"),
        Control::check(Camera, "Post Processing", "AomacHdr", "HDR Sunlight and Tonemap"),
        Control::slider(Camera, "Post Processing", "AomacGrade", "Colour Grade (warm sun, cool shade)", "%.2f", 0.8, 0.0, 1.0),
        Control::check(Camera, "Antialiasing", "AomacTaa", "Temporal Antialiasing"),
        Control::check(Camera, "Antialiasing", "AomacFxaa", "FXAA (when TAA is off)"),
        Control::check(Camera, "Sharpening", "AomacCas", "Contrast Adaptive Sharpening"),
        Control::slider(Camera, "Sharpening", "AomacCasStrength", "Sharpening Strength", "%.2f", 0.5, 0.0, 1.0),
        Control::slider(Camera, "Motion Blur", "AomacMotionBlur", "Motion Blur", "%.2f", 0.0, 0.0, 1.0),
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
            items.push(Item::Text { value: "Enhanced effects are individually opt-in.".into(), borders: "Rect(0,5,0,5)".into() });
            items.push(Item::Text { value: "Choose a category to adjust its settings.".into(), borders: "Rect(0,0,0,5)".into() });
            items.push(Item::Text { value: "Restore All resets every Enhanced control.".into(), borders: "Rect(0,0,0,10)".into() });
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
    for control in controls().into_iter().filter(|control| page.is_none_or(|page| page == control.page)) {
        d.set(control.name, control.default);
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
            assert!(d.set(control.name, changed(control)));
        }
        let saved = d.save_config(CAT_MAIN);
        let mut loaded = values();
        assert!(loaded.load_config(&saved, CAT_MAIN, false));
        for control in &controls {
            assert_eq!(loaded.get(control.name), Some(&changed(control)));
        }
        for page in PageId::ALL {
            for control in &controls {
                loaded.set(control.name, changed(control));
            }
            reset(&mut loaded, Some(page));
            for control in &controls {
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
}
