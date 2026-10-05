//! Renderer-agnostic scene description shared by `ao-formats` (producer) and `ao-render` (consumer).
//!
//! Conventions: right-handed, +Y up, units are AO world units (1.0 = 1 metre).
//! Matrices are column-major `[[f32; 4]; 4]` (`m[col][row]`), applied as `M * v`.

pub mod aurora;
pub mod mover;
pub mod wave;
pub use wave::{sun_flicker_table, wave_curves, SkyWaveSpin};
pub use mover::{Counter, Mover, MoverState, RotTerm};

use std::collections::HashMap;

/// Decoded texture, tightly packed RGBA8 (sRGB), row-major, top row first.
#[derive(Clone, Debug)]
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// Linear RGBA multiplier on the lit texel (baked shadow/lighting, terrain blend weight in `a`).
    pub color: [f32; 4],
}

pub const WHITE: [f32; 4] = [1.0; 4];

impl Default for Vertex {
    fn default() -> Self {
        Self { pos: [0.0; 3], normal: [0.0, 1.0, 0.0], uv: [0.0; 2], color: WHITE }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Blend {
    #[default]
    Opaque,
    /// Cutout at alpha 0.5, depth-written (foliage, fences).
    AlphaTest,
    /// Src-alpha blending, drawn after opaques back-to-front, no depth write (glass, water).
    AlphaBlend,
    /// Additive (src*alpha + dst), no depth write (glows, beams).
    Additive,
}

/// Triangle list sharing the parent mesh's vertex buffer.
#[derive(Clone, Debug)]
pub struct Submesh {
    pub indices: Vec<u32>,
    /// Key into [`Scene::textures`]; `None` draws `base_color` only.
    pub texture: Option<TextureKey>,
    pub blend: Blend,
    /// Linear RGBA tint multiplied with the texture (and the vertex colour). Archive meshes and CAT models always carry white RGB +
    /// `opac`: `RViewPort_t::SetMaterial` (randy31 @0x1004b199) never copies the material's `diff`/`ambi` RGB into the
    /// `_D3DMATERIAL7`, whose diffuse and ambient stay white (`SetDefaultMaterial` @0x1004b61e).
    pub base_color: [f32; 4],
    /// Disable back-face culling (front faces are counter-clockwise in ao-scene space).
    pub two_sided: bool,
    /// `_D3DMATERIAL7.emissive` = the material's `emis` (linear RGB; `EMISSIVEMATERIALSOURCE` = MATERIAL). D3D adds it to the vertex
    /// lighting sum, which is then saturated: `tex * saturate(emissive + ambient + diffuse lights)`, then fog.
    pub emissive: [f32; 3],
    /// Texture alpha is a self-illumination mask (opaque materials only): the texel is
    /// lit by `max(lighting, min(lighting + a, 1))` (engine: `saturate(a + lighting)`).
    pub glow_mask: bool,
    /// Baked-lighting surface (dungeon room shells, shadow-mapped ground): `Vertex::color.rgb` is an *additive* light in D3D
    /// (gamma) space, not linear: `lit = tex * tint * saturate(vertex.rgb + 0.8 * ambient + point/spot diffuse)` (all gamma
    /// space); no sun, no specular. Fog still applies.
    pub prelit: bool,
    /// Texture coordinate drift in uv units per second (added to the vertex uv, wrapped by the sampler): scrolling clouds,
    /// water-like layers. `[0, 0]` = static.
    pub uv_scroll: [f32; 2],
    /// Sky submeshes only: vertex `normal.x` is the distance (metres, at the view distance) the vertex has in the client's
    /// fogged atmosphere strip; the renderer fogs it with the live fog (colour / end at the camera) instead of a baked one.
    pub sky_fog: bool,
    /// Sky submeshes only: `[curve_u, amp_u, curve_v, amp_v]`, the uv offset `amp * GameWaveCurve<curve>(time)`
    /// ([`wave_curves`]) added to the vertex uv every frame (a tweak `ScrollMatrix` translation driven by `GAME.GameWaveCurve*`;
    /// the constant part is baked into the uv). `amp == 0` = off.
    pub uv_wave: [f32; 4],
    /// Sky submeshes only (`e_SunRays`): vertex `normal.z` is the fan rim index `i` (`-1` = centre); the renderer scales the rim
    /// alpha by `(255 - table[i & 7]) / 255` with `table` = [`sun_flicker_table`] (`FUN_1005a3a9`).
    pub sun_flicker: bool,
    /// `_D3DMATERIAL7.specular` = `spec * shin_str` (linear RGB) while `SPECULARENABLE` holds, else zero (= no specular term). Added
    /// after the texture stage, per vertex: `specular * Σ light_colour * (N.H)^shininess * atten` for lights with `N.L > 0`.
    pub specular: [f32; 3],
    /// `_D3DMATERIAL7.power` (`shin`).
    pub shininess: f32,
}

impl Submesh {
    pub fn new(indices: Vec<u32>, texture: Option<TextureKey>) -> Self {
        Self {
            indices,
            texture,
            blend: Blend::Opaque,
            base_color: WHITE,
            two_sided: false,
            emissive: [0.0; 3],
            glow_mask: false,
            prelit: false,
            uv_scroll: [0.0; 2],
            sky_fog: false,
            uv_wave: [0.0; 4],
            sun_flicker: false,
            specular: [0.0; 3],
            shininess: 0.0,
        }
    }
}

/// Dynamic-style light (world space), the fixed function D3D7 light of the original client (`D3DLIGHT7`).
///
/// Intensity at distance `d <= range` is `1 / (atten0 + atten1 d + atten2 d^2)` (D3D attenuation), 0 beyond `range`;
/// with `atten == [0; 3]` it is the linear ramp `1 - d / range`. A [`Spot`] multiplies the D3D cone factor.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Light {
    pub pos: [f32; 3],
    /// Linear RGB (the D3D diffuse colour).
    pub color: [f32; 3],
    pub range: f32,
    /// `D3DLIGHT7.dvAttenuation0..2`; all zero = linear falloff to zero at `range`.
    pub atten: [f32; 3],
    pub spot: Option<Spot>,
    /// Statel zone the light belongs to (`Scene::statel_lod`): it only shines while [`StatelLod::lights_active`] holds for
    /// that zone's level. `None` = always on.
    pub zone: Option<u32>,
}

/// `D3DLIGHT_SPOT` cone: full inner angle `theta` and outer angle `phi` (radians, `dvTheta` / `dvPhi`), falloff 1:
/// factor 1 inside `theta`, 0 outside `phi`, linear in `cos(angle / 2)` between.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spot {
    /// Unit axis (world space).
    pub dir: [f32; 3],
    pub theta: f32,
    pub phi: f32,
}

#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub submeshes: Vec<Submesh>,
}

/// Placement of `Scene::meshes[mesh]` in world space.
#[derive(Clone, Copy, Debug)]
pub struct Instance {
    pub mesh: usize,
    pub transform: [[f32; 4]; 4],
}

/// Texture identity: the rdb record (type, id) it was decoded from, or a synthetic id
/// for textures generated at load time (e.g. baked terrain) using `rdb_type = 0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureKey {
    pub rdb_type: u32,
    pub id: u32,
}

/// Per-playfield atmosphere; linear RGB.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Environment {
    pub sky_color: [f32; 3],
    pub fog_color: [f32; 3],
    /// Fog is 0 at `fog_start` metres, full at `fog_end`.
    pub fog_start: f32,
    pub fog_end: f32,
    pub ambient: [f32; 3],
    pub sun_color: [f32; 3],
    /// Unit vector pointing from the scene towards the sun.
    pub sun_dir: [f32; 3],
    /// `SpecularLightIntensity` of the sun tweak: the sun `D3DLIGHT7` specular colour is `sun_specular * sun_color`
    /// (DisplaySystem `FUN_10059b98` @0x10059b98). 1.0 in every default tweak.
    pub sun_specular: f32,
}

/// Perspective lens of the camera (`VisualCamera_t(fov, aspect, near, far)` -> `RCamera_t`, randy31 @0x1002a68a).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lens {
    /// Field of view in radians. `RCamera_t` takes `tan(fov / 2)` as the half width of the view plane at distance 1 and
    /// derives the half height by dividing it by the aspect: the angle is **horizontal**, see [`Lens::vertical_fov`].
    pub fov: f32,
    /// `true` = `fov` is the horizontal angle (the original engine), `false` = vertical.
    pub horizontal: bool,
    pub near: f32,
    /// `None` = the renderer picks the far plane from the fog distance.
    pub far: Option<f32>,
}

impl Lens {
    /// Vertical field of view for a viewport of `aspect` = width / height.
    pub fn vertical_fov(&self, aspect: f32) -> f32 {
        if self.horizontal { 2.0 * ((self.fov * 0.5).tan() / aspect).atan() } else { self.fov }
    }
}

impl Default for Lens {
    /// The free-fly viewer / playfield default: 60 degrees vertical, near 0.2.
    fn default() -> Self {
        Self { fov: std::f32::consts::FRAC_PI_3, horizontal: false, near: 0.2, far: None }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub textures: HashMap<TextureKey, Texture>,
    pub meshes: Vec<Mesh>,
    pub instances: Vec<Instance>,
    /// Suggested initial camera position (world space); `None` = frame the scene bounds.
    pub spawn: Option<[f32; 3]>,
    /// Point the initial camera looks at; `None` = renderer's choice.
    pub spawn_look_at: Option<[f32; 3]>,
    /// `None` = renderer defaults.
    pub environment: Option<Environment>,
    /// `None` = [`Lens::default`].
    pub lens: Option<Lens>,
    pub lights: Vec<Light>,
    /// Sky domes/backdrops: instances of `meshes` drawn first, depth-write off, unfogged,
    /// with the transform's translation replaced by the camera position. Not in `instances`.
    pub sky: Vec<Instance>,
    /// Dynamic fog: the playfield's base fog plus the local fog volumes of the statel file; the renderer re-evaluates it
    /// at the camera every frame and overrides `environment.fog_color` / `fog_end` (and the clear colour when it equals
    /// the fog colour). `None` = the static environment fog.
    pub fog_model: Option<FogModel>,
    /// Distance dependent statel visibility, see [`StatelLod`]. `None` = every instance is always drawn.
    pub statel_lod: Option<StatelLod>,
    /// Constant rotations of `sky` instances (the Shadowlands vortex: a `Counter` that grows with `GameDeltaTime`).
    pub sky_spin: Vec<SkySpin>,
    /// Rotations of `sky` instances driven by `GameWaveCurve*` ([`SkyWaveSpin`]).
    pub sky_wave_spin: Vec<SkyWaveSpin>,
    /// Vertex colours of `meshes` that the renderer re-simulates every frame (the Shadowlands aurora, [`aurora`]).
    pub sky_colors: Vec<SkyColors>,
    /// Traffic ships: instances of `instances` whose transform the renderer advances every frame ([`Mover`]).
    pub movers: Vec<Mover>,
    /// Game day time (seconds, 0..6480) the scene was built at; the movers' clock starts here.
    pub day_time: f32,
}

/// `meshes[mesh]` gets its vertex colours from `sim` (ARGB map entry per vertex, vertex `i` = map texel `i`), advanced to
/// the renderer's time every frame. Colour = `min(rgb * gain, 1)` (D3D `MODULATE2X/4X`) in linear space, alpha = a / 255.
#[derive(Clone, Debug, PartialEq)]
pub struct SkyColors {
    pub mesh: usize,
    pub sim: aurora::GloomySky,
    pub gain: f32,
}

/// A sky instance that turns about `axis` (right-handed scene space, unit) through the point `pivot` (relative to the
/// camera) at `degrees_per_second`; the renderer composes it with the instance transform every frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkySpin {
    /// Index into [`Scene::sky`].
    pub instance: usize,
    pub axis: [f32; 3],
    pub pivot: [f32; 3],
    pub degrees_per_second: f32,
}

/// Local fog volume (`n3StatelFog_t`, statel file; `StatelFogRun` N3 @0x10024dbc): inside `radius` metres of `pos` the
/// client adds fog `color` with density `density * (1 - (d / radius)^4)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FogVolume {
    pub pos: [f32; 3],
    /// Gamma space RGB 0..1.
    pub color: [f32; 3],
    /// 0..1 (the file stores percent).
    pub density: f32,
    pub radius: f32,
    /// Dungeon room the volume belongs to: it is only evaluated while that room is the camera's room (see [`FogModel::rooms`]).
    pub room: Option<u32>,
}

/// The client's per-frame fog accumulation (`VisualFog_t::AddFog` DisplaySystem @0x1005820c, `process` @0x10058443).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FogModel {
    /// Playfield fog after the record/atmosphere `AddFog` calls: gamma space RGB and density 0..1.
    pub base_color: [f32; 3],
    pub base_density: f32,
    pub near: f32,
    /// View distance (fog end at density 0).
    pub far: f32,
    pub volumes: Vec<FogVolume>,
    /// Dungeon room lookup (`None` outdoors). The client evaluates a room's fog/sound lists only while `n3RoomMonitor_t`
    /// has the room enabled (N3 `RunFunction` @0x100260d2): here the camera's room, found like
    /// `n3Playfield_t::PosToRoom` (first room whose `IsPosInside` holds, else room 0). Rooms that are also
    /// enabled through open doors depend on the server's door state and are not modelled.
    pub rooms: Option<RoomLocator>,
}

/// Scene-space position -> first dungeon room containing it (`n3Room_t::IsPosInside`), supplied by the loader.
#[derive(Clone)]
pub struct RoomLocator(pub std::sync::Arc<dyn Fn([f32; 3]) -> Option<usize> + Send + Sync>);

impl std::fmt::Debug for RoomLocator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RoomLocator")
    }
}

impl PartialEq for RoomLocator {
    fn eq(&self, o: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &o.0)
    }
}

impl FogModel {
    /// Room containing scene-space `p` (room 0 when none does); `None` outdoors.
    pub fn camera_room(&self, p: [f32; 3]) -> Option<usize> {
        self.rooms.as_ref().map(|r| (r.0)(p).unwrap_or(0))
    }

    /// Fog at camera position `p`: `(linear RGB, fog end in metres)`. `AddFog` keeps a weighted mean
    /// `new = (d * c_new + D * c_old) / (d + D)` and the density `D = max(D, d)`; volumes are added in file order.
    pub fn at(&self, p: [f32; 3]) -> ([f32; 3], f32) {
        let (mut c, mut dens) = (self.base_color, self.base_density);
        let room = self.camera_room(p);
        for v in self.volumes.iter().filter(|v| v.room.is_none_or(|r| Some(r as usize) == room)) {
            let d2: f32 = (0..3).map(|i| (p[i] - v.pos[i]).powi(2)).sum();
            let r2 = v.radius * v.radius;
            if v.radius > 0.0 && d2 < r2 {
                let d = v.density * (1.0 - (d2 / r2) * (d2 / r2));
                if d + dens > 0.0 {
                    c = std::array::from_fn(|i| (d * v.color[i] + dens * c[i]) / (d + dens));
                }
                dens = dens.max(d);
            }
        }
        let end = if self.far - self.near > 5.0 { self.far - (self.far - self.near - 5.0) * dens } else { self.far };
        (c.map(|v| v.powf(2.2)), end)
    }
}

pub const IDENTITY: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn horizontal_fov_shrinks_the_vertical_angle_with_the_aspect() {
        let l = Lens { fov: 60f32.to_radians(), horizontal: true, near: 0.5, far: Some(1000.0) };
        // 4:3: tan(v/2) = tan(30 deg) * 3/4
        assert!((l.vertical_fov(4.0 / 3.0).to_degrees() - 46.827).abs() < 1e-2);
        assert!((l.vertical_fov(1.0).to_degrees() - 60.0).abs() < 1e-4);
        assert!((Lens::default().vertical_fov(2.0).to_degrees() - 60.0).abs() < 1e-4);
    }

    fn model() -> FogModel {
        FogModel { base_color: [0.2; 3], base_density: 0.1, near: 0.5, far: 800.0, volumes: vec![FogVolume { pos: [0.0; 3], color: [1.0, 0.0, 0.0], density: 0.9, radius: 100.0, room: None }], rooms: None }
    }

    #[test]
    fn fog_end_follows_density_and_volume_centre_wins() {
        let m = model();
        // outside the volume: base fog only, end = far - (far - near - 5) * D
        let (c, end) = m.at([200.0, 0.0, 0.0]);
        assert!((end - (800.0 - 794.5 * 0.1)).abs() < 1e-3 && (c[0] - 0.2f32.powf(2.2)).abs() < 1e-6);
        // centre: d = 0.9 -> mean (0.9 * red + 0.1 * grey) = 0.92 red, density 0.9
        let (c, end) = m.at([0.0; 3]);
        assert!((end - (800.0 - 794.5 * 0.9)).abs() < 1e-3 && (c[0] - 0.92f32.powf(2.2)).abs() < 1e-5 && (c[1] - 0.02f32.powf(2.2)).abs() < 1e-5);
        // quartic falloff: at half the radius d = 0.9 * (1 - 1/16)
        let (_, end) = m.at([50.0, 0.0, 0.0]);
        assert!((end - (800.0 - 794.5 * 0.9 * (1.0 - 0.0625))).abs() < 1e-2);
    }

    fn lod() -> StatelLod {
        let item = |class, flag8, reduced, zones: Vec<u32>| LodItem { full: 0, reduced, flag8, class, zones };
        StatelLod::new(800.0, vec![[0.0, 0.0], [1000.0, 0.0]], vec![item(0, true, Some(1), vec![0]), item(3, false, None, vec![0]), item(4, true, Some(1), vec![0, 1]), item(1, false, None, vec![0])])
    }

    #[test]
    fn room_fog_is_only_evaluated_in_the_camera_room() {
        let vol = |room, x| FogVolume { pos: [x, 0.0, 0.0], color: [1.0, 0.0, 0.0], density: 0.9, radius: 50.0, room };
        let m = FogModel {
            base_color: [0.2; 3],
            base_density: 0.1,
            near: 0.5,
            far: 800.0,
            volumes: vec![vol(Some(1), 100.0)],
            // room 0 around x = 0, room 1 around x = 100
            rooms: Some(RoomLocator(std::sync::Arc::new(|p| [0.0f32, 100.0].iter().position(|c| (p[0] - c).abs() < 20.0)))),
        };
        let base_end = m.at([0.0, 0.0, 0.0]).1;
        assert_eq!(m.camera_room([100.0, 0.0, 0.0]), Some(1));
        // inside the volume's radius but in room 0: ignored; in room 1: applied
        assert!((m.at([60.0, 0.0, 0.0]).1 - base_end).abs() < 1e-3 || m.camera_room([60.0, 0.0, 0.0]) == Some(0));
        assert!(m.at([100.0, 0.0, 0.0]).1 < base_end);
        assert_eq!(FogModel::default().camera_room([0.0; 3]), None);
    }

    #[test]
    fn zone_lights_shine_only_in_the_middle_levels() {
        assert_eq!([0, 1, 2, 3, 4, 5].map(StatelLod::lights_active), [false, true, true, true, false, false]);
    }

    #[test]
    fn zone_levels_follow_the_client_bands() {
        // half view length 400: radii 40, 60, 120, 160, 220 (the 0.1 factor hits the 40 m floor exactly)
        let l = lod();
        let at = |d: f32| l.level(0, [d, 0.0]);
        assert_eq!([at(10.0), at(50.0), at(100.0), at(140.0), at(200.0), at(300.0)], [1, 2, 3, 4, 5, 0]);
        // a short view length keeps the 40 m floor for every band
        let s = StatelLod::new(100.0, vec![[0.0, 0.0]], vec![]);
        assert_eq!([s.level(0, [39.0, 0.0]), s.level(0, [41.0, 0.0])], [1, 0]);
    }

    #[test]
    fn statel_modes_follow_the_zone_state_table() {
        let l = lod();
        let pick = |i: usize, levels: [u8; 2], prev: bool| l.pick(&l.items[i], &levels, prev);
        // file list 3 -> zone list 0 (small clutter): only within the nearest band
        assert_eq!([pick(1, [1, 0], false).0, pick(1, [2, 0], false).0, pick(1, [0, 0], false).0], [LodPick::Full, LodPick::Hidden, LodPick::Hidden]);
        // file list 0 -> zone list 3, flag 8: full out to level 4, mode 1 (reduced) at level 5, mode 2 (reduced) at level 0
        assert_eq!([pick(0, [4, 0], false).0, pick(0, [5, 0], false).0, pick(0, [0, 0], false).0], [LodPick::Full, LodPick::Reduced, LodPick::Reduced]);
        // file list 1 -> zone list 2, no flag 8 / reduced record: mode 1 (level 4) keeps what it showed last, mode 2 (5) finds no record
        assert_eq!(pick(3, [3, 0], false), (LodPick::Full, false));
        assert_eq!(pick(3, [4, 0], false), (LodPick::Full, false));
        assert_eq!(pick(3, [5, 0], false), (LodPick::Hidden, true));
        assert_eq!(pick(3, [4, 0], true), (LodPick::Hidden, true));
        // global statel (zone list 4): state 2 (mode 1) only at level 0; the best mode of its zones wins
        assert_eq!((pick(2, [5, 5], false).0, pick(2, [0, 0], false).0, pick(2, [0, 1], false).0), (LodPick::Full, LodPick::Reduced, LodPick::Full));
        assert_eq!(l.zone_items, vec![vec![0, 1, 2, 3], vec![2]]);
    }
}

/// Which representation of a statel is shown (`FUN_100241ab`, N3 @0x100241ab).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LodPick {
    Hidden,
    /// `Identity{0xf6951, id}`: the full mesh (rdb 1010001).
    Full,
    /// `Identity{0xf696a, id}`: the reduced mesh (rdb 1010026).
    Reduced,
}

/// One statel under distance control: its full-mesh instance, the optional reduced-mesh twin (both are entries of
/// `Scene::instances`, the renderer shows at most one of them) and the zones that reference it.
#[derive(Clone, Debug, PartialEq)]
pub struct LodItem {
    pub full: usize,
    pub reduced: Option<usize>,
    /// Statel flag bit 3: the reduced mesh may replace the full one in mode 1 (`extraout_ECX[8]` of `FUN_1002435c`).
    pub flag8: bool,
    /// Zone list class 0..4 (`StatelLod::STATE` columns; 4 = global statel referenced through a zone's index list).
    pub class: u8,
    /// Indices into `StatelLod::zones` (one for zone statels, every referencing zone for global statels).
    pub zones: Vec<u32>,
}

/// The client's statel zone LOD (`n3StatelController_t`, N3.dll). Every frame `FUN_10028ca6` gives each zone a level 0..5
/// from the distance between the camera and the zone centre (`FUN_10028ab7`); `FUN_100286a9` then sets a state per
/// statel list of the zone (`STATE`), `FUN_10028577` turns states into modes (1 -> 2, 2 -> 1, 3 -> 0, 0 = disabled) and
/// `FUN_1002431a` shows each statel in the best (lowest) mode any referencing zone requests.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatelLod {
    /// `VisualCamera_t::GetLengthOfViewcone` = far - near of the camera (metres).
    pub view_length: f32,
    /// Zone centres `[x, z]` in scene space.
    pub zones: Vec<[f32; 2]>,
    pub items: Vec<LodItem>,
    /// Item indices per zone (derived by [`StatelLod::new`]).
    pub zone_items: Vec<Vec<u32>>,
}

impl StatelLod {
    /// Distances of the levels as fractions of half the view length (`n3StatelController` zone manager +0x2c..+0x3c,
    /// floats 0.1, 0.15, 0.3, 0.4, 0.55 at N3 @0x1003d61c, 0x1003e674, 0x1003e29c, 0x1003e670, 0x1003e66c).
    pub const FACTORS: [f32; 5] = [0.1, 0.15, 0.3, 0.4, 0.55];
    /// Lower bound of every distance (float 40.0 at N3 @0x1003e668).
    pub const MIN_DISTANCE: f32 = 40.0;
    /// `STATE[level][zone list]`, zone lists 0..5 = (file statel list 3, 2, 1, 0, global refs, lights); `FUN_100286a9`.
    pub const STATE: [[u8; 6]; 6] = [[0, 0, 0, 1, 2, 0], [3, 3, 3, 3, 3, 3], [0, 3, 3, 3, 3, 3], [0, 0, 3, 3, 3, 3], [0, 0, 2, 3, 3, 0], [0, 0, 1, 2, 3, 0]];

    /// Zone list 5 (the lights) is enabled (state 3) only in levels 1..=3 (`STATE[level][5]`).
    pub fn lights_active(level: u8) -> bool {
        Self::STATE[level as usize][5] != 0
    }

    pub fn new(view_length: f32, zones: Vec<[f32; 2]>, items: Vec<LodItem>) -> Self {
        let mut zone_items = vec![Vec::new(); zones.len()];
        for (i, it) in items.iter().enumerate() {
            for &z in &it.zones {
                zone_items[z as usize].push(i as u32);
            }
        }
        Self { view_length, zones, items, zone_items }
    }

    /// `FUN_10028ab7`: level of a zone for a camera at `cam` (x, z).
    pub fn level(&self, zone: usize, cam: [f32; 2]) -> u8 {
        let c = self.zones[zone];
        let d2 = (cam[0] - c[0]).powi(2) + (cam[1] - c[1]).powi(2);
        let half = self.view_length * 0.5;
        let r = Self::FACTORS.map(|f| (half * f).max(Self::MIN_DISTANCE).powi(2));
        if d2 < r[0] {
            1
        } else if d2 < r[1] {
            2
        } else if d2 < r[2] {
            3
        } else if d2 < r[3] {
            4
        } else if d2 < r[4] {
            5
        } else {
            0
        }
    }

    /// What `item` shows when its zones are at `levels`, given whether its current identity is the reduced mesh
    /// (`ident_reduced`, the controller's `+0x14` word, initially the full mesh); returns the pick and the new identity.
    /// `FUN_100241ab` changes the identity only for mode 0 (full), mode 1 with flag 8 and mode 2 (reduced): a statel
    /// without flag 8 at mode 1, or one that was hidden, keeps whatever it showed last.
    pub fn pick(&self, item: &LodItem, levels: &[u8], ident_reduced: bool) -> (LodPick, bool) {
        // state -> mode (`local_18` of FUN_10028577: 1 -> 2, 2 -> 1, 3 -> 0); the statel uses the lowest mode requested
        let mode = item
            .zones
            .iter()
            .filter_map(|&z| match Self::STATE[levels[z as usize] as usize][[3, 2, 1, 0, 4][item.class as usize]] {
                0 => None,
                1 => Some(2),
                2 => Some(1),
                _ => Some(0),
            })
            .min();
        let Some(mode) = mode else { return (LodPick::Hidden, ident_reduced) };
        let reduced = match mode {
            0 => false,
            1 if item.flag8 => true,
            1 => ident_reduced,
            _ => true,
        };
        // a reduced identity without a 1010026 record creates no visual
        let pick = match (reduced, item.reduced.is_some()) {
            (false, _) => LodPick::Full,
            (true, true) => LodPick::Reduced,
            (true, false) => LodPick::Hidden,
        };
        (pick, reduced)
    }
}
