//! Character creation (`CharCreateModule_t`, `SceneBase_t` and the four scenes of GUI.dll; RE evidence in
//! `docs/screens.md` §12): the 3D world, the camera rig of `CharCreateCamera.dat`, the CC breed / profession tables and
//! the name rules.
//!
//! Coordinates: functions marked *AO* use AO's left-handed Y-up space, everything that ends up in a [`Scene`] is renderer
//! space (Z negated, [`crate::screens::ao_to_render`]).

use crate::character::NameTable;
use crate::mesh::{decode_mesh_connectors, decode_mesh_into, decode_mesh_lights, MESH_TYPE};
use crate::screens::{login_environment, CameraPose, CharCreateCameras};
use anyhow::{Context, Result};
use ao_rdb::RecordStore;
use ao_scene::{Instance, Lens, Scene, IDENTITY};
use std::collections::HashMap;

/// The six meshes `CharCreateModule_t::InitialiseMessage` loads in this order (table at GUI 0x102724e4). Index = slot in
/// the module (`this+0x68 + 4·i`): 0 main (station interior, the 7 breed connectors), 1 professions, 2 seq01 (intro
/// sequence far away at x ≈ −1490), 3 professions02, 4 nanoeffect, 5 adventurer.
pub const CC_MESHES: [&str; 6] = [
    "charactercreation_main.abiff",
    "charactercreation_professions.abiff",
    "charactercreation_seq01.abiff",
    "charactercreation_professions02.abiff",
    "charactercreation_nanoeffect.abiff",
    "charactercreation_adventurer.abiff",
];

/// Slots of [`CC_MESHES`] that `CharCreateModule_t::FrameProcess` shows only while `this[0xaa]` is set (professions,
/// professions02, nanoeffect, adventurer): set when the profession scene starts, cleared by the intro (0x44d).
pub const PROFESSION_MESHES: [usize; 4] = [1, 3, 4, 5];

/// The 7 CC breeds in `SlotMeshReady`'s table (GUI 0x10272490: `Breed_e, BreedSex_e, connector`), CC breed id = index + 1.
/// Breed 1 Solitus 2 Opifex 3 Nanomage 4 Atrox; sex 2 male 3 female.
pub const CC_BREEDS: [(i32, i32, &str); 7] =
    [(1, 3, "breed_0"), (1, 2, "breed_1"), (2, 3, "breed_2"), (2, 2, "breed_3"), (3, 3, "breed_4"), (3, 2, "breed_5"), (4, 2, "breed_6")];

/// `SceneBase_t::ConvertCCBreedToGCBreedAndSex` (GUI 0x101225f7): CC breed 1..7 → `(Breed_e, BreedSex_e)`.
pub fn cc_breed_to_gc(cc: i32) -> Option<(i32, i32)> {
    CC_BREEDS.get(usize::try_from(cc).ok()?.checked_sub(1)?).map(|b| (b.0, b.1))
}

/// `SceneBase_t::ConvertCCProfToGCProf` (GUI 0x1012256e): CC profession 1..14 → `GameData::Profession_e`
/// (13 = unknown / none). 1 Metaphysicist 2 Adventurer 3 Engineer 4 Soldier 5 Keeper 6 Shade 7 Fixer 8 Agent 9 Trader
/// 10 Doctor 11 Enforcer 12 Bureaucrat 13 Martial Artist 14 Nano-Technician.
pub fn cc_prof_to_gc(cc: i32) -> i32 {
    match cc {
        1 => 12,
        2 => 6,
        3 => 3,
        4 => 1,
        5 => 14,
        6 => 15,
        7 => 4,
        8 => 5,
        9 => 7,
        10 => 10,
        11 => 9,
        12 => 8,
        13 => 2,
        14 => 11,
        _ => 13,
    }
}

/// text.mdb (category 600) key stems of the 14 CC professions, index = CC profession − 1 (`Inspect<k>`, `<k>Selected`,
/// `Description<k>`; the metaphysicist description key is `DescriptionMetaPhysicists`).
pub const PROF_KEYS: [&str; 14] = [
    "Metaphysicist",
    "Adventurer",
    "Engineer",
    "Soldier",
    "Keeper",
    "Shade",
    "Fixer",
    "Agent",
    "Trader",
    "Doctor",
    "Enforcer",
    "Bureaucrat",
    "MartialArtist",
    "NanoTechnician",
];

/// `SM_Sandy_CC_*` sound of CC profession k (ctor order of `ProfessionScene_t`, GUI 0x10121225).
pub const PROF_SOUNDS: [&str; 14] = [
    "SM_Sandy_CC_Meta_Selected",
    "SM_Sandy_CC_Adventurer",
    "SM_Sandy_CC_Engineer",
    "SM_Sandy_CC_Soldier",
    "SM_Sandy_CC_Keeper",
    "SM_Sandy_CC_Shade",
    "SM_Sandy_CC_Fixer",
    "SM_Sandy_CC_Agent",
    "SM_Sandy_CC_Trader",
    "SM_Sandy_CC_Doctor",
    "SM_Sandy_CC_Enforcer",
    "SM_Sandy_CC_Bureaucrat",
    "SM_Sandy_CC_MA",
    "SM_Sandy_CC_Nanotech_Selected",
];

/// Placement of a [`crate::character`] model at a connector, AO space: `CCCharacter_t::MoveToConnector(mesh, name, true)`
/// (GUI 0x1011ae66): position = connector translation, rotation = `M · Rx(π) · Ry(−0.5)` of its 3×3 (row vectors, the
/// engine's quaternion→matrix convention). Returned as a 4×4 (rotation + translation).
pub fn character_at_connector(m: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut r = [[0.0; 3]; 3];
    let rx = [[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, -1.0]];
    let (s, c) = (-0.5f32).sin_cos();
    let ry = [[c, 0.0, -s], [0.0, 1.0, 0.0], [s, 0.0, c]];
    let mul3 = |a: [[f32; 3]; 3], b: [[f32; 3]; 3]| -> [[f32; 3]; 3] {
        let mut o = [[0.0; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                o[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
            }
        }
        o
    };
    for (i, row) in r.iter_mut().enumerate() {
        row.copy_from_slice(&m[i][..3]);
    }
    let r = mul3(mul3(r, rx), ry);
    [[r[0][0], r[0][1], r[0][2], 0.0], [r[1][0], r[1][1], r[1][2], 0.0], [r[2][0], r[2][1], r[2][2], 0.0], [m[3][0], m[3][1], m[3][2], 1.0]]
}

/// AO-space row-vector matrix → renderer space for an instance transform (`S · M · S`, S = diag(1,1,−1)).
pub fn ao_matrix_to_render(m: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut o = *m;
    for (i, row) in o.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            if (i == 2) != (j == 2) {
                *v = -*v;
            }
        }
    }
    o
}

/// The static 3D world of the creation screens: the six meshes (all at the origin, lit by their `RLight_t` nodes only,
/// like the login backdrop) and the connectors of `charactercreation_main.abiff` (AO space).
pub struct CcWorld {
    /// `meshes`/`instances` in [`CC_MESHES`] order (instance i = mesh slot i).
    pub scene: Scene,
    pub connectors: HashMap<String, [[f32; 4]; 4]>,
}

/// Camera of the module before the first transition (`FUN_10117b77`: position (0, 1, −5), identity rotation) and the
/// lens: `VisualCamera_t(fov, aspect, 0.5, 1000)` (GUI 0x1011ba81), horizontal FOV like every `RCamera_t`.
pub const CC_NEAR: f32 = 0.5;
pub const CC_FAR: f32 = 1000.0;

pub fn cc_world(store: &RecordStore) -> Result<CcWorld> {
    let names = NameTable::load(store)?;
    let mut scene = Scene::default();
    let mut connectors = HashMap::new();
    for (i, n) in CC_MESHES.iter().enumerate() {
        let id = names.id(MESH_TYPE, n).with_context(|| format!("no mesh {n}"))?;
        let mesh = decode_mesh_into(store, id, &mut scene)?.with_context(|| format!("mesh {n} missing"))?;
        scene.instances.push(Instance { mesh, transform: IDENTITY });
        scene.lights.extend(decode_mesh_lights(store, MESH_TYPE, id, [0.0; 3])?);
        if i == 0 {
            connectors.extend(decode_mesh_connectors(store, MESH_TYPE, id)?);
        }
    }
    scene.environment = Some(login_environment());
    scene.lens = Some(Lens { fov: std::f32::consts::FRAC_PI_3, horizontal: true, near: CC_NEAR, far: Some(CC_FAR) });
    Ok(CcWorld { scene, connectors })
}

// ---------------------------------------------------------------------------------------------------------------
// camera rig
// ---------------------------------------------------------------------------------------------------------------

/// One camera pose (AO space, quaternion x y z w, horizontal FOV in degrees).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub pos: [f32; 3],
    pub rot: [f32; 4],
    pub fov_deg: f32,
}

impl Pose {
    /// View direction (+Z rotated by the quaternion).
    pub fn forward(&self) -> [f32; 3] {
        let [x, y, z, w] = self.rot;
        [2.0 * (x * z + w * y), 2.0 * (y * z - w * x), 1.0 - 2.0 * (x * x + y * y)]
    }
}

impl From<&CameraPose> for Pose {
    fn from(c: &CameraPose) -> Self {
        Pose { pos: c.pos, rot: c.rot, fov_deg: c.fov_deg }
    }
}

struct Transition {
    id: Vec<u32>,
    duration: f32,
    keys: Vec<Vec<u32>>,
    /// Identity of this definition (`FUN_101163ee` compares the node pointer with the current one).
    node: u64,
}

fn slerp(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let mut d = a.iter().zip(&b).map(|(x, y)| x * y).sum::<f32>();
    let mut b = b;
    if d < 0.0 {
        d = -d;
        b = b.map(|v| -v);
    }
    let (wa, wb) = if d > 0.9995 {
        (1.0 - t, t)
    } else {
        let th = d.clamp(-1.0, 1.0).acos();
        (((1.0 - t) * th).sin() / th.sin(), (t * th).sin() / th.sin())
    };
    let q = [0, 1, 2, 3].map(|i| a[i] * wa + b[i] * wb);
    let n = q.iter().map(|v| v * v).sum::<f32>().sqrt();
    q.map(|v| v / n)
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|i| (a[i] - b[i]) * (a[i] - b[i])).sum::<f32>().sqrt()
}

/// The camera path player of `CharCreateModule_t` (`FUN_1011663e` @GUI 0x1011663e, the camera tool of
/// `CharCreateCamera.dat`): a transition walks the polyline of its key cameras by arc length with the ease
/// `s = (1 − cos(π p)) / 2` (`p` = elapsed / duration), position and FOV interpolate linearly and rotation spherically
/// inside the segment; every frame the result is low-passed into the camera: `cam = new·(1−b) + cam·b`,
/// `b = 0.935 − min(dt, 0.035)`.
pub struct CameraRig {
    cams: HashMap<Vec<u32>, Pose>,
    trans: Vec<Transition>,
    next_node: u64,
    cur: Option<u64>,
    progress: f32,
    active: bool,
    pose: Pose,
}

impl CameraRig {
    pub fn new(c: &CharCreateCameras) -> Self {
        let mut rig = CameraRig {
            cams: c.cameras.iter().map(|p| (p.id.clone(), Pose::from(p))).collect(),
            trans: vec![],
            next_node: 0,
            cur: None,
            progress: 0.0,
            active: false,
            pose: Pose { pos: [0.0, 1.0, -5.0], rot: [0.0, 0.0, 0.0, 1.0], fov_deg: 60.0 },
        };
        for t in &c.transitions {
            rig.set_transition(&t.id, t.duration_s, t.keyframes.clone());
        }
        rig
    }

    /// `FUN_10115987` + `FUN_10116b7a`: (re)defines the transition `id`.
    pub fn set_transition(&mut self, id: &[u32], duration: f32, keys: Vec<Vec<u32>>) {
        self.trans.retain(|t| t.id != id);
        self.next_node += 1;
        self.trans.push(Transition { id: id.to_vec(), duration, keys, node: self.next_node });
    }

    /// `FUN_10115903`: replaces the pose of camera `id`.
    pub fn set_camera(&mut self, id: &[u32], pose: Pose) {
        self.cams.insert(id.to_vec(), pose);
    }

    pub fn camera(&self, id: &[u32]) -> Option<Pose> {
        self.cams.get(id).copied()
    }

    /// `FUN_101163ee` + `FUN_10116248(1)`: starts transition `id` unless it is the one already current (same definition).
    pub fn start(&mut self, id: &[u32]) {
        let Some(t) = self.trans.iter().find(|t| t.id == id) else { return };
        if self.cur != Some(t.node) {
            self.cur = Some(t.node);
            self.progress = 0.0;
            self.active = true;
        }
    }

    /// Puts the camera on camera `id` at once (no transition).
    pub fn jump(&mut self, id: &[u32]) {
        if let Some(p) = self.cams.get(id) {
            self.pose = *p;
            self.active = false;
        }
    }

    /// Camera-tool command 0x31 (`SlotEscPressed`, GUI 0x1011660a → `FUN_10116248(count)`): while a transition plays, it
    /// stops and the camera snaps to the transition's last key camera; otherwise nothing happens.
    pub fn stop(&mut self) {
        if self.active {
            if let Some(t) = self.target() {
                self.pose = t;
            }
            self.active = false;
        }
    }

    pub fn pose(&self) -> Pose {
        self.pose
    }

    /// Duration in seconds of the current transition (`FUN_10115541`), 0 when none.
    pub fn duration(&self) -> f32 {
        self.trans.iter().find(|t| Some(t.node) == self.cur).map_or(0.0, |t| t.duration)
    }

    /// Normalised progress of the running transition.
    pub fn progress(&self) -> f32 {
        self.progress
    }

    fn keys(&self, t: &Transition) -> Vec<Pose> {
        t.keys.iter().filter_map(|k| self.cams.get(k).copied()).collect()
    }

    /// Target of the current transition (its last key camera).
    pub fn target(&self) -> Option<Pose> {
        let t = self.trans.iter().find(|t| Some(t.node) == self.cur)?;
        self.keys(t).last().copied()
    }

    /// `FUN_101153ee`: no transition running, or the camera is within 0.1 of the path's end.
    pub fn done(&self) -> bool {
        !self.active || self.target().is_none_or(|t| dist(t.pos, self.pose.pos) < 0.1)
    }

    /// One frame (`dt` seconds).
    pub fn update(&mut self, dt: f32) {
        if !self.active {
            return;
        }
        let Some(t) = self.trans.iter().find(|t| Some(t.node) == self.cur) else { return };
        let keys = self.keys(t);
        if keys.is_empty() {
            return;
        }
        self.progress = (self.progress + dt / t.duration).min(1.0);
        let e = (1.0 - (std::f32::consts::PI * self.progress).cos()) * 0.5;
        let seg: Vec<f32> = keys.windows(2).map(|w| dist(w[0].pos, w[1].pos)).collect();
        let total: f32 = seg.iter().sum();
        let mut new = keys[0];
        if total > 0.0 && self.progress >= 1e-4 {
            let mut acc = 0.0;
            for (i, d) in seg.iter().enumerate() {
                acc += d;
                let frac = acc / total;
                if e < frac || i == seg.len() - 1 {
                    let start = (acc - d) / total;
                    let s = if *d > 0.0 { ((e - start) / (d / total)).clamp(0.0, 1.0) } else { 1.0 };
                    let (a, b) = (keys[i], keys[i + 1]);
                    new = Pose {
                        pos: [0, 1, 2].map(|k| a.pos[k] * (1.0 - s) + b.pos[k] * s),
                        rot: slerp(a.rot, b.rot, s),
                        fov_deg: a.fov_deg * (1.0 - s) + b.fov_deg * s,
                    };
                    break;
                }
            }
        }
        let b = 0.935 - dt.min(0.035);
        let c = 1.0 - b;
        let p = self.pose;
        self.pose = Pose {
            pos: [0, 1, 2].map(|k| new.pos[k] * c + p.pos[k] * b),
            rot: slerp(p.rot, new.rot, c),
            fov_deg: new.fov_deg * c + p.fov_deg * b,
        };
        if self.progress >= 1.0 && dist(self.pose.pos, keys[keys.len() - 1].pos) < 0.001 {
            self.active = false;
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// name rules
// ---------------------------------------------------------------------------------------------------------------

/// `FUN_10123be2` (GUI 0x10123be2): the typed name as the client normalises it before checking and sending: first
/// letter upper case, the rest lower case, every character that is neither a letter nor a digit dropped.
pub fn normalize_name(raw: &str) -> String {
    raw.chars()
        .enumerate()
        .map(|(i, c)| if i == 0 { c.to_ascii_uppercase() } else { c.to_ascii_lowercase() })
        .filter(char::is_ascii_alphanumeric)
        .collect()
}

/// Why a name is refused (`FUN_1012382b` @0x1012382b codes → `NameScene_t::StartServerCreation` messages).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameError {
    /// Not 4..=12 characters: text key `NameMustBeBetween3And13Chars`.
    Length,
    /// A control character: `NameMustBeAlphanum`.
    Alphanumeric,
    /// Digit in the first four characters or a letter after a digit: `DigitsAtEndAfter4`.
    Digits,
}

impl NameError {
    /// text.mdb category 600 key.
    pub fn key(self) -> &'static str {
        match self {
            NameError::Length => "NameMustBeBetween3And13Chars",
            NameError::Alphanumeric => "NameMustBeAlphanum",
            NameError::Digits => "DigitsAtEndAfter4",
        }
    }
}

/// `FUN_1012382b` on the [`normalize_name`]d string.
pub fn check_name(name: &str) -> Result<(), NameError> {
    let b = name.as_bytes();
    if !(4..=12).contains(&b.len()) {
        return Err(NameError::Length);
    }
    let mut digit = false;
    for (i, &c) in b.iter().enumerate() {
        if c.is_ascii_digit() {
            if i < 4 {
                return Err(NameError::Digits);
            }
            digit = true;
        } else if !c.is_ascii_alphabetic() {
            // codes 2 (printable) and 5 (control character) show the same message
            return Err(NameError::Alphanumeric);
        } else if digit {
            return Err(NameError::Digits);
        }
        // code 3 (wrong letter case) cannot occur after `normalize_name` and has no message in `StartServerCreation`
    }
    Ok(())
}

/// `NameScene_t::SlotTextInput` (GUI 0x1011f619): a typed character is rejected unless it is a letter or digit.
pub fn name_char_ok(c: char) -> bool {
    c.is_ascii_alphanumeric()
}

/// `NameScene_t::SetState(code)` error codes → text.mdb category 600 key (GUI 0x1011f75e).
pub fn name_state_key(code: i32) -> Option<&'static str> {
    Some(match code {
        1 => "PlayerNotFound",
        2 => "IllegalPassword",
        3 => "ContactAdministration",
        4 => "CharacterNotFound",
        5 => "AccountHasIllegalPassword",
        6 => "CharacterAlreadyLoggedIn",
        7 => "CharacterProblem",
        8 => "CantCreateMoreChars",
        9 => "ServerProblem",
        10 => "ServerDown",
        0x13 => "AccountNotPaid",
        0x14 => "PlayerAlreadyLoggedIn",
        0x15 => "AccountLocked",
        0x1c => "ServerWrongRDB",
        0x1e => "NicknameTaken",
        0x1f => "NicknameInvalid",
        _ => return None,
    })
}

/// Height in percent on the wire for `CCSelectedHeight` 0/1/2 (`NameScene_t::SetState(0x1006)`: `0 → 90`, `1 → 100`,
/// otherwise 110) and the model scale of `CCCharacter_t::RunFunction` (0.95 / 1.0 / 1.05).
pub fn height_percent(h: i32) -> i32 {
    match h {
        0 => 90,
        1 => 100,
        _ => 110,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_rules() {
        assert_eq!(normalize_name("jOHN-doe"), "Johndoe");
        assert_eq!(check_name("Abc"), Err(NameError::Length));
        assert_eq!(check_name("Abcdefghijklm"), Err(NameError::Length));
        assert_eq!(check_name("Abcd"), Ok(()));
        assert_eq!(check_name("Ab1de"), Err(NameError::Digits));
        assert_eq!(check_name("Abcde1"), Ok(()));
        assert_eq!(check_name("Abcde1f"), Err(NameError::Digits));
    }

    #[test]
    fn breed_and_profession_tables() {
        assert_eq!(cc_breed_to_gc(1), Some((1, 3)));
        assert_eq!(cc_breed_to_gc(7), Some((4, 2)));
        assert_eq!(cc_breed_to_gc(0), None);
        assert_eq!(cc_prof_to_gc(4), 1);
        assert_eq!(cc_prof_to_gc(0), 13);
        let mut all: Vec<_> = (1..=14).map(cc_prof_to_gc).collect();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), 14);
    }

    fn rig() -> CameraRig {
        let cams = "CameraCount: 2\nID: 1\nPos: 0 0 0\nRot: 0 0 0 1\ndegFOV: 60\nID: 2\nPos: 10 0 0\nRot: 0 0 0 1\ndegFOV: 90\n\
                    TransitionCount: 1\nTransitionId: 1\nDuration: 2 sec\nKeyframeCount: 2\nC: 1\nC: 2\n";
        CameraRig::new(&CharCreateCameras::parse(cams).unwrap())
    }

    #[test]
    fn rig_walks_the_path_and_settles() {
        let mut r = rig();
        r.start(&[1]);
        assert!(!r.done());
        let mut prev = -1.0;
        for _ in 0..600 {
            r.update(1.0 / 60.0);
            let x = r.pose().pos[0];
            assert!(x >= prev - 1e-4, "monotonic");
            prev = x;
        }
        assert!(r.done());
        assert!((r.pose().pos[0] - 10.0).abs() < 0.01 && (r.pose().fov_deg - 90.0).abs() < 0.1);
    }

    #[test]
    fn restarting_the_same_definition_is_ignored_until_redefined() {
        let mut r = rig();
        r.start(&[1]);
        for _ in 0..10 {
            r.update(0.1);
        }
        let p = r.progress();
        r.start(&[1]);
        assert_eq!(r.progress(), p);
        r.set_transition(&[1], 1.0, vec![vec![2], vec![1]]);
        r.start(&[1]);
        assert_eq!(r.progress(), 0.0);
    }

    #[test]
    fn connector_rotation_flips_the_model() {
        let m = character_at_connector(&[[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [1.0, 2.0, 3.0, 1.0]]);
        assert_eq!(&m[3][..3], &[1.0, 2.0, 3.0]);
        assert!(m[1][1] < -0.99, "Rx(pi) flips Y");
    }
}
