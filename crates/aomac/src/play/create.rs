//! The original character creation (`CharCreateModule_t`, `SceneBase_t` and the Breed / Appearance / Profession / Name
//! scenes of GUI.dll; RE evidence and every address in `docs/screens.md` §12).
//!
//! The 3D world is the `charactercreation_*.abiff` set; a [`CameraRig`] flies through `CharCreateCamera.dat`; the scenes'
//! widgets are what the original builds in code (3-state image buttons placed by display fractions, `MakeLabel` banners,
//! the framed HTML text area, the name `TextInputView_c`).

mod actor;
mod frame;
mod scenes;
#[cfg(test)]
mod shots;

use super::*;
use ao_formats::character::{self, Breed, Gender};
use ao_formats::create::*;
use ao_formats::screens::{ao_to_render, CharCreateCameras};
use ao_net::msg::CreateCharacterRequest;
use ao_scene::Lens;
use actor::{Actor, Spec};
use glam::Vec3 as V3;

/// Scene ids (`SceneBase_t` +0x28 = 0x3e9..0x3ec as 0..3).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Sc {
    Breed,
    Appearance,
    Profession,
    Name,
}

impl Sc {
    fn index(self) -> usize {
        self as usize
    }
    fn from(i: usize) -> Sc {
        [Sc::Breed, Sc::Appearance, Sc::Profession, Sc::Name][i]
    }
}

/// `CharCreateModule_t::FrameProcess` states (`this+0x4c`).
#[derive(Clone, Copy, PartialEq, Debug)]
enum St {
    /// 0x44d: `RunIntro`.
    Intro,
    /// 0x44e: the space fly-by, camera transition [5 1].
    IntroWait,
    /// 0x3e9..0x3ec: scene chosen, state moves on at once.
    Pick(Sc),
    /// 0x3f3..0x3f6: waiting for the camera before `StartScene`.
    Wait(Sc),
    /// 0: a scene is active.
    Active,
    /// 0x4b1: leaving, the shuttle fly-away [4 3].
    Exit,
    /// Quit confirmed: back to character selection.
    Gone,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum B {
    Exit,
    Next,
    Back,
    HeadPrev,
    HeadNext,
    /// height 0/1/2
    Height(i32),
    /// build 0/1/2
    Build(i32),
    Suggest,
    Finish,
    Prof(i32),
    /// Full-screen invisible button of the breed scene (3D picking).
    BreedBg,
    /// Invisible hot zone around the head of the appearance scene.
    HeadArea,
}

struct Btn {
    kind: B,
    /// normal, pressed, over (`Button_c::SetGfx(0/1/2, …)` as `MakeButton(normal, pressed, over)`).
    gfx: [GfxId; 3],
    rect: [f32; 4],
    enabled: bool,
    invisible: bool,
}

/// `PlainSpriteFade_t` (black full-screen sprite): alpha runs `a0 → a1` over `dur` seconds.
struct Fade {
    t: f32,
    dur: f32,
    a0: f32,
    a1: f32,
    /// `StaticDoneTimerCallback` id fired when it ends.
    done: Option<u8>,
}

/// `TextWindowFade_t(fade, hold, text, font 4, x, y)`: alpha 0→1 in `fade`, 1 for `hold`, 1→0 in `fade`.
struct TextFade {
    t: f32,
    fade: f32,
    hold: f32,
    text: String,
    pos: (f32, f32),
}

/// One-shot timer (`FUN_10008501(seconds)` + callback id).
struct Timer {
    left: f32,
    id: u8,
}

pub(super) struct Create {
    dir: PathBuf,
    world: Option<CcWorld>,
    rig: CameraRig,
    actors: Vec<Actor>,
    st: St,
    cur: Option<Sc>,
    /// `this+0x2c/0x24` of `SceneBase_t`: `StopScene(code)` hides the scene and reports `code` after 0.5 s.
    stop: Option<(f32, i32)>,
    // CC prefs (IndependentPrefs `CCSelected*`)
    breed: i32,
    height: i32,
    size: i32,
    head: usize,
    prof: i32,
    // module flags (`this+0xa9` .. `0xad`)
    breed_vis: bool,
    prof_vis: bool,
    texts_shown: bool,
    fading: bool,
    ambience: bool,
    // scene ui
    shown: bool,
    btns: Vec<Btn>,
    hover: Option<usize>,
    pressed: Option<usize>,
    info: (String, String),
    banner: String,
    labels: Vec<(String, f32, f32)>,
    info_rect: [f32; 4],
    info_w: Option<WindowId>,
    name_w: Option<WindowId>,
    // breed scene
    hover_breed: i32,
    last_hover_breed: i32,
    // appearance scene
    head_state: i32,
    // name scene
    pending_suggest: bool,
    name_locked: bool,
    pending_appearance: Option<CreateCharacterRequest>,
    heads: Vec<character::HeadEntry>,
    // animation / effects
    fades: Vec<Fade>,
    text_fades: Vec<TextFade>,
    timers: Vec<Timer>,
    uploaded: Vec<bool>,
    scene_dirty: bool,
    mouse: (f32, f32),
    opening_music: Vec<u64>,
    exit_t: f32,
    /// `CharacterCreated` arrived / ZoneHandoff pending.
    created: bool,
    pub(super) handoff: bool,
    /// A notice box (`NameScene_t::Message`) is up; the scene returns when it is closed.
    message_open: bool,
}

fn rot_vec(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    // active rotation q v q^-1 (the convention of `Pose::forward`)
    let [x, y, z, w] = q;
    let t = [2.0 * (y * v[2] - z * v[1]), 2.0 * (z * v[0] - x * v[2]), 2.0 * (x * v[1] - y * v[0])];
    [v[0] + w * t[0] + (y * t[2] - z * t[1]), v[1] + w * t[1] + (z * t[0] - x * t[2]), v[2] + w * t[2] + (x * t[1] - y * t[0])]
}

fn bool_env(k: &str) -> bool {
    std::env::var_os(k).is_some()
}

impl Create {
    fn new(dir: PathBuf, rig: CameraRig, p: &prefs::CcPrefs) -> Create {
        Create {
            dir,
            world: None,
            rig,
            actors: vec![],
            st: St::Intro,
            cur: None,
            stop: None,
            breed: p.breed,
            height: p.height,
            size: p.size,
            head: p.head.max(0) as usize,
            prof: p.profession,
            breed_vis: true,
            prof_vis: true,
            texts_shown: false,
            fading: false,
            ambience: false,
            shown: false,
            btns: vec![],
            hover: None,
            pressed: None,
            info: Default::default(),
            banner: String::new(),
            labels: vec![],
            info_rect: [0.0; 4],
            info_w: None,
            name_w: None,
            hover_breed: 0,
            last_hover_breed: 0,
            head_state: 0,
            pending_suggest: false,
            name_locked: false,
            pending_appearance: None,
            heads: vec![],
            fades: vec![],
            text_fades: vec![],
            timers: vec![],
            uploaded: vec![],
            scene_dirty: true,
            mouse: (0.0, 0.0),
            opening_music: vec![],
            exit_t: 0.0,
            created: false,
            handoff: false,
            message_open: false,
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// head table
// ---------------------------------------------------------------------------------------------------------------

/// `CCCharacter_t::MakeHeadMeshTable` (GUI 0x1011aacb): the head table of the (breed, sex) as `FUN_1011d368` builds it, in
/// insertion order (`character::head_table`). The table is built once, when `CharSelectWindow_c` (created by
/// `LoginModule_c::SlotInitialize` before the character list) constructs its first `CCCharacter_t`, so DValue
/// `ExpansionFlags` is still 0 (docs/screens.md § 12).
fn head_table(dir: &std::path::Path, breed: Breed, gender: Gender) -> Vec<character::HeadEntry> {
    let Ok(store) = RecordStore::open(dir) else { return vec![] };
    character::head_table(&store, breed, gender, 0).unwrap_or_default()
}

fn gc_breed(b: i32, s: i32) -> Option<(Breed, Gender)> {
    screens::wire_breed_sex(b, s).ok()
}

impl Create {
    fn spec(&self, breed_idx: usize, head: usize, build: i32) -> Option<Spec> {
        let (b, s, _) = CC_BREEDS[breed_idx];
        let (breed, gender) = gc_breed(b, s)?;
        let table = if self.heads.is_empty() { head_table(&self.dir, breed, gender) } else { self.heads.clone() };
        let h = table.get(head).or(table.first())?;
        Some(Spec { breed, gender, head: (h.num, h.skin), build: build.clamp(0, 2) as u8 })
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Play integration
// ---------------------------------------------------------------------------------------------------------------

impl Play {
    fn cc_text(&self, key: &str) -> String {
        self.text.by_key(600, key).unwrap_or_default()
    }

    fn cc_sound(&self, name: &str) -> Vec<u64> {
        self.audio.as_ref().map(|a| a.play_ui(name)).unwrap_or_default()
    }

    /// New Character pressed (`CharSelectWindow_c::SlotCreatePressed` → `LoginModule_c::SlotCreateChar`): the creation
    /// module takes over.
    pub(super) fn start_creation(&mut self, host: &mut Host) {
        self.close_all();
        self.dialog_w = None;
        self.screen = Screen::Create;
        self.worker = None;
        host.fly = false;
        let cams = CharCreateCameras::load(&self.dir).map_err(|e| eprintln!("CharCreateCamera.dat: {e:#}"));
        let Ok(cams) = cams else { return self.back_to_selection(host) };
        let p = self.prefs.cc.clone();
        let mut c = Create::new(self.dir.clone(), CameraRig::new(&cams), &p);
        // `InitialiseMessage`: a creation that finished last time resets the selections
        if self.prefs.cc_created {
            c.breed = 0;
            c.height = 1;
            c.size = 1;
            c.head = 0;
            c.prof = 0;
            self.prefs.cc_created = false;
            self.prefs.cc = prefs::CcPrefs { breed: 0, height: 1, size: 1, head: 0, profession: 0 };
            self.prefs.save();
        }
        match self.cc_world.take() {
            Some(w) => c.world = Some(*w),
            None => {
                let (tx, dir) = (self.tx.clone(), self.dir.clone());
                std::thread::spawn(move || {
                    let r = RecordStore::open(&dir).and_then(|s| cc_world(&s)).map(Box::new).map_err(|e| format!("{e:#}"));
                    let _ = tx.send(Bg::CcWorld(r));
                });
            }
        }
        self.cc = Some(Box::new(c));
        self.cc_try_init(host);
    }

    pub(super) fn cc_world_loaded(&mut self, r: Result<Box<CcWorld>, String>, host: &mut Host) {
        match (r, self.cc.as_mut()) {
            (Ok(w), Some(c)) => c.world = Some(*w),
            (Ok(w), None) => self.cc_world = Some(w),
            (Err(e), _) => {
                eprintln!("character creation world: {e}");
                self.message_box(&format!("Trying to create a new character using wrong datasett? Check that you got Live/14.5/14.6/new2player_exp . ({e})"));
                return self.back_to_selection(host);
            }
        }
        self.cc_try_init(host);
    }

    /// `InitialiseMessage` second half, once the meshes are there: the characters, the first state.
    fn cc_try_init(&mut self, host: &mut Host) {
        let Some(mut c) = self.cc.take() else { return };
        if c.world.is_none() || !c.actors.is_empty() {
            self.cc = Some(c);
            return;
        }
        // 7 breed characters at the connectors (head 0, build 1, height 1), then the module's own character
        let conn = c.world.as_ref().map(|w| w.connectors.clone()).unwrap_or_default();
        for (i, (_, _, name)) in CC_BREEDS.iter().enumerate() {
            if let Some(spec) = c.spec(i, 0, 1) {
                let mut a = Actor::new(self.dir.clone(), spec, i as u32 + 1);
                a.place = conn.get(*name).copied();
                c.actors.push(a);
            }
        }
        let main_breed = (c.breed - 1).clamp(0, 6) as usize;
        let (b, s, _) = CC_BREEDS[main_breed];
        if let Some((breed, gender)) = gc_breed(b, s) {
            c.heads = head_table(&self.dir, breed, gender);
        }
        if let Some(spec) = c.spec(main_breed, c.head, c.size) {
            let mut a = Actor::new(self.dir.clone(), spec, 0);
            a.height = c.height;
            a.place = conn.get("character_0").copied();
            c.actors.insert(0, a);
        }
        c.uploaded = vec![false; c.actors.len()];
        c.st = St::Intro;
        if bool_env("AOMAC_CC_SKIP_INTRO") {
            // verification aid: jump to the breed scene (the original has no skip)
            c.rig.jump(&[1, 1]);
            c.st = St::Pick(Sc::Breed);
        }
        self.cc = Some(c);
        let _ = host;
    }

    fn back_to_selection(&mut self, host: &mut Host) {
        self.cc_close_windows();
        self.cc = None;
        self.screen = Screen::CharSelect;
        let list = ao_net::msg::CharacterList { characters: self.chars.clone(), allowed_characters: self.slots, ..Default::default() };
        host.lens = Some(screens::LOGIN_LENS);
        self.show_characters(list, host);
    }

    pub(super) fn cc_close_windows(&mut self) {
        if let Some(c) = self.cc.as_mut() {
            for w in [c.info_w.take(), c.name_w.take()].into_iter().flatten() {
                self.gui.close_window(w);
            }
        }
    }
}

impl Create {
    /// `SlotEscPressed`: camera-tool command 0x31, see [`CameraRig::stop`].
    pub(super) fn esc(&mut self) {
        self.rig.stop();
    }

    /// The exit cinematic reached the loading hand-over (`StaticDoneTimerCallback` 4: `AFCM::Send(0x12, 0x3d)`).
    pub(super) fn exit_done(&self) -> bool {
        self.st == St::Exit && self.created && self.handoff
    }
}
