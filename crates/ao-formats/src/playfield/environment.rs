//! Per-playfield environment: the `GameData::EnvironmentData_t` that follows the liquid list in the
//! playfield record tail (rdb 1000001, see `water`).
//!
//! ```text
//! u32 0, u32 0 (always zero in the 601 records), u32 size (=30 in all records)
//! 26 bytes (stream order = EnvironmentData_t byte index 0..9, 0xb, 0xa, 0xc..0x19; `operator>>` GameData @0x1000b711,
//!           3 more bytes when size != 0x1a, one more when size != 0x1d)
//! ```
//! Consumers (Gamecode.dll): `FUN_100bd183` copies bytes `0..7` and `8..0x12` to the sound/weather object and bytes
//! `0x13..0x19` to the atmosphere object that `FUN_100bd661` (per frame) reads as
//! `AddAmbientLight(b[0..3]/255)` and `VisualFog_t::AddFog(b[3..6]/255, b[6]/100)` (divisors are the doubles
//! 255.0 @0x10166f60 and 100.0 @0x10158670). With the playfield bytes `0x13..0x19` that is
//! * ambient light colour = `b[0x13..0x16] / 255`
//! * fog colour = `b[0x16..0x19] / 255`, fog density = `b[0x19] / 100`
//!
//! When the fog colour is black the client falls back to colour 0.2 grey and density 0.01. `VisualFog_t::process`
//! (DisplaySystem @0x10058443) turns density `D` into the fog end `far - (far - near - 5) * D` (clip planes from
//! `VisualFog_t::AddClipPlanes`), and `Randy_t::SetFogParameters` (randy31.dll @0x10041876) programs Direct3D
//! linear vertex fog `[near, end]` (render state 0x1c on, mode 3 = linear; the density argument is not used).

use anyhow::{ensure, Result};
use ao_scene::Environment;

use super::record::Rd;
use super::sky::{Sky, ATMOSPHERE_FOG_DENSITY};

/// Bytes of the on-disk `EnvironmentData_t` for `size == 30`.
const LEN: usize = 26;
/// Fog / ambient fields start here.
const AMBIENT: usize = 0x13;

/// Far clip plane = `ViewDistance` pref * 1000 m (`FUN_1001fc91` N3 @0x1001fc91, DisplaySystem `AddClipPlanes`); the pref is a 0..1 slider whose
/// default is 0.8 (`SetDefaultLoginPrefs` GUI @0x10124b33: `InitDefaultFloat("ViewDistance", 0.8, 0, 1)`).
pub(super) const VIEW_DISTANCE: f32 = 800.0;
/// Near clip plane = start of the linear fog.
pub(super) const NEAR: f32 = 0.5;
/// Day sky (clear) colour, sRGB. The client's sky dome is time-of-day driven (`GfxVisualSkyrise`, not stored per playfield).
const SKY_SRGB: [f32; 3] = [0.53, 0.72, 0.92];
const SUN_DIR: [f32; 3] = [0.4, 0.8, 0.3];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Env {
    pub raw: [u8; LEN],
}

impl Env {
    pub fn ambient(&self) -> [u8; 3] {
        self.raw[AMBIENT..AMBIENT + 3].try_into().unwrap()
    }
    pub fn fog_color(&self) -> [u8; 3] {
        self.raw[AMBIENT + 3..AMBIENT + 6].try_into().unwrap()
    }
    /// Fog density, 0..=1 (percent / 100).
    pub fn fog_density(&self) -> f32 {
        self.raw[AMBIENT + 6] as f32 / 100.0
    }
}

pub fn parse(r: &mut Rd) -> Result<Env> {
    let (a, b, size) = (r.u32()?, r.u32()?, r.u32()?);
    ensure!(a == 0 && b == 0, "unexpected non-zero environment header {a} {b}");
    ensure!(size == 30, "unsupported EnvironmentData size {size}");
    let mut raw = [0u8; LEN];
    raw.copy_from_slice(r.take(LEN)?);
    Ok(Env { raw })
}

pub fn srgb_to_linear(c: f32) -> f32 {
    c.powf(2.2)
}

fn lin(c: [u8; 3]) -> [f32; 3] {
    c.map(|b| srgb_to_linear(b as f32 / 255.0))
}

/// `Scene::environment` for a playfield. `outdoor`: sun lit terrain (colours from the tweak scripts in `sky`, else
/// fixed guesses). The fog colour is the density weighted mix of the record's `AddFog` and the atmosphere's
/// (`VisualFog_t::AddFog` @0x1005820c: `new = (d * c_new + D * c_old) / (d + D)`, `D = max`).
/// The playfield fog after the record's and the atmosphere's `AddFog` calls: gamma space RGB and density.
fn base_fog(env: &Env, sky: Option<&Sky>) -> ([f32; 3], f32) {
    let fog = env.fog_color();
    let (c1, d1) = if fog == [0, 0, 0] || env.fog_density() <= 0.0 { ([0.2; 3], 0.01) } else { (fog.map(|b| b as f32 / 255.0), env.fog_density()) };
    match sky {
        Some(s) => {
            let d = d1 + ATMOSPHERE_FOG_DENSITY;
            ([0, 1, 2].map(|k| (c1[k] * d1 + s.fog[k] * ATMOSPHERE_FOG_DENSITY) / d), d1.max(ATMOSPHERE_FOG_DENSITY))
        }
        None => (c1, d1),
    }
}

/// Per-frame fog of a playfield: the base fog plus the statel file's local fog volumes (`Scene::fog_model`).
pub fn fog_model(env: &Env, sky: Option<&Sky>, volumes: Vec<ao_scene::FogVolume>) -> ao_scene::FogModel {
    let (base_color, base_density) = base_fog(env, sky);
    ao_scene::FogModel { base_color, base_density, near: NEAR, far: VIEW_DISTANCE, volumes, rooms: vec![] }
}

pub fn to_scene(env: &Env, outdoor: bool, sky: Option<&Sky>) -> Environment {
    let (fog_srgb, density) = base_fog(env, sky);
    let fog_color = fog_srgb.map(srgb_to_linear);
    // `VisualFog_t::process`: the end only moves when far - near > 5, by (far - near - 5) * D
    let fog_end = VIEW_DISTANCE - (VIEW_DISTANCE - NEAR - 5.0) * density;
    let a = lin(env.ambient());
    let (ambient, sun_color) = match (outdoor, sky) {
        (true, s) => {
            let amb = s.and_then(|s| s.ambient).map_or(a, |m| [0, 1, 2].map(|k| a[k].max(srgb_to_linear(m[k]))));
            (amb, s.and_then(|s| s.sun).map_or(a.map(|v| 1.0 - v), |c| c.map(srgb_to_linear)))
        }
        // `Tweak_Rubi-Ka_IndoorLight` AmbientLightCurrent 0.01 competes with the record ambient (`AddAmbientLight` keeps the
        // per-channel maximum, DisplaySystem @0x10059d2c); no sun indoors; the room lightmaps carry the lighting
        (false, _) => (a.map(|v| v.max(0.01)), [0.0; 3]),
    };
    // indoors the clear colour is black (`Tweak_BlackBackground`); outdoors it is the fog colour: everything at the far
    // distance, the dome's lowest ring included, is fully fogged, so the clear colour must equal it (no band below the horizon)
    let sky_color = match (outdoor, sky) {
        (true, Some(_)) => fog_color,
        (true, None) => SKY_SRGB.map(srgb_to_linear),
        _ => [0.0; 3],
    };
    let dir = sky.map_or(SUN_DIR, |s| s.sun_dir);
    let l = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
    Environment { sky_color, fog_color, fog_start: NEAR, fog_end, ambient, sun_color, sun_dir: dir.map(|v| v / l) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(ambient: [u8; 3], fog: [u8; 3], density: u8) -> Vec<u8> {
        let mut d = vec![0u8; 8];
        d.extend(30u32.to_le_bytes());
        let mut raw = [0u8; LEN];
        raw[AMBIENT..AMBIENT + 3].copy_from_slice(&ambient);
        raw[AMBIENT + 3..AMBIENT + 6].copy_from_slice(&fog);
        raw[AMBIENT + 6] = density;
        d.extend(raw);
        d
    }

    #[test]
    fn decodes_fields_and_fog_range() {
        let d = fixture([50, 50, 50], [68, 232, 107], 15); // Varmint Woods: green fog, 15 %
        let mut r = Rd::new(&d, 0);
        let e = parse(&mut r).unwrap();
        assert_eq!(r.o, d.len());
        assert_eq!((e.ambient(), e.fog_color(), e.fog_density()), ([50; 3], [68, 232, 107], 0.15));
        let s = to_scene(&e, true, None);
        assert!((s.fog_end - (VIEW_DISTANCE - (VIEW_DISTANCE - NEAR - 5.0) * 0.15)).abs() < 1e-3);
        assert!(s.fog_color[1] > s.fog_color[0] && s.fog_color[1] > s.fog_color[2]);
    }

    #[test]
    fn black_fog_falls_back_to_default_density() {
        let e = parse(&mut Rd::new(&fixture([30; 3], [0; 3], 50), 0)).unwrap();
        let s = to_scene(&e, true, None);
        assert!((s.fog_end - (VIEW_DISTANCE - (VIEW_DISTANCE - NEAR - 5.0) * 0.01)).abs() < 1e-3);
        assert_eq!(s.fog_color, [srgb_to_linear(0.2); 3]);
        assert!(to_scene(&e, false, None).ambient[0] >= 0.01);
    }

    #[test]
    fn rejects_unknown_size() {
        let mut d = fixture([0; 3], [0; 3], 0);
        d[8] = 26;
        assert!(parse(&mut Rd::new(&d, 0)).is_err());
    }
}
