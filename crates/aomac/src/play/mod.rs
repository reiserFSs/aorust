//! `aomac play`: the original login flow (docs/screens.md) -- login window, progress dialog, character selection with the
//! 3D `LoginWorld_c` backdrop and `CharacterViewer_c` preview, loading screen, then the zone's playfield.
//! The GUI is `ao_gui` (the client's own view XML + skin); the network is `ao_net::client`.

mod avatar;
mod camera;
mod camera_views;
#[cfg(test)]
mod sky_shots;
mod chat;
mod combat;
mod controls;
mod create;
mod delete;
mod dvalue;
mod dynels;
mod dynels_doors;
mod fightmode;
mod flow;
mod hud;
mod hud_help;
mod hud_tips;
mod level_notice;
mod hud_aggdef;
mod hud_actions;
mod hud_actionwin;
mod hud_bar;
mod hud_compass;
mod hud_dialog;
mod hud_faction;
mod hud_perks;
mod hud_research_vehicle;
mod vehicle_guard;
mod hud_pet;
mod hud_team;
mod hud_wincfg;
mod hud_winb;
mod hud_cursor;
mod hud_pick;
mod hud_map;
mod hud_mission;
mod hud_listview;
mod hud_nano;
mod lightning;
mod hud_keys;
mod hud_nanodb;
mod hud_ncu;
mod hud_knowledge;
mod references;
mod hud_target_ncu;
mod hud_notification;
mod hud_pools;
mod hud_rollup;
mod hud_special;
mod hud_stats;
mod hud_target;
mod hud_timerbar;
mod hud_use;
mod interact;
mod interact_chat;
mod interact_grid;
mod interact_loot;
mod interact_mission;
mod interact_play;
mod interact_ptrade;
mod interact_shop;
mod interact_bank;
mod interact_trade;
mod interact_tradeskill;
mod interact_tower;
mod interact_use;
mod logout;
mod options;
mod own_nanos;
mod movement;
mod native_life;
mod native_life_spawn;
mod native_life_render;
mod player;
mod prefs;
mod preview;
mod tags;
mod zone;

use anyhow::Result;
use ao_audio::Audio;
use ao_formats::screens::{self, TextDb};
use ao_formats::create::CcWorld;
use ao_gui::{DrawCmd, DrawList, Event, FontId, Gui, GfxId, InputEvent, Key, MouseButton, WindowId, WindowSize};
use ao_net::client::{LoginEvent, LoginSession, ServerEntry};
use ao_net::msg::{CharacterEntry, CharacterList};
use ao_rdb::RecordStore;
use ao_render::{Camera, Frontend, Host, Vec3};
use ao_scene::{Mesh, Scene};
use prefs::Prefs;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};

/// `PlayfieldProxy::playfield` identity type (`IdentityType.Playfield`, protocol.md §6): `instance` is the playfield id.
const PLAYFIELD_IDENTITY: i32 = 0xC79D;
/// `GeneralNetworkTimeout` / `DimensionNetworkTimeout` of MainPrefs.xml (docs/screens.md §8.1).
const CONNECT_TIMEOUT: f32 = 90.0;
const JOIN_TIMEOUT: f32 = 30.0;
/// `ServerLogin3DModule_t` fades (GUI 0x101aa198 / 0x101aa190); the unit (ms) is an assumption (docs/screens.md §7).
const FADE_IN: f32 = 4.0;
const FADE_OUT: f32 = 7.0;
/// `TeleportEndedMessage` [GUI 0x100292ce]: signed frame counter and float seconds (0x101ae4d8).
const IN_PLAY_FRAMES: i32 = 10;
const IN_PLAY_SECONDS: f32 = 120.0;
/// Camera height above the player's feet; [GUESS] same eye height as the playfield spawn heuristic (1.7 m), the client's player camera is not ported yet.
const EYE_HEIGHT: f32 = 1.7;

/// The playfield map's ground image and the terrain cell size in metres (`None`: dungeon), see `hud_map::ground_map`.
type WorldGround = (ao_formats::topdown::GroundMap, Option<f32>);

enum Bg {
    Servers(Result<Vec<ServerEntry>, String>),
    Connected(u32, Result<LoginSession, String>),
    Backdrop(Result<Box<Scene>, String>),
    World(u32, Option<u32>, i32, Result<(Box<Scene>, bool), String>),
    GroundTextures(u64, Result<HashMap<ao_scene::TextureKey, ao_scene::Texture>, String>),
    /// The live sky source of playfield `id` (`None`: indoor / no tweak script), sent just before its [`Bg::World`].
    Sky(u32, Box<Option<ao_formats::playfield::SkyClock>>),
    /// Per-frame effect wind, loaded independently of the audio device.
    Weather(u32, Box<ao_formats::weather::Weather>),
    Life(u32, Result<Box<(native_life_spawn::LifeWorld, native_life_render::LifeModels)>, String>),
    /// The top-down ground image of playfield `id` for the Map window (`Report::ground` rendered by `topdown::render` from the scene
    /// the loader just built; `bool`: dungeon rooms), sent just before its [`Bg::World`].
    Ground(u32, Option<Box<WorldGround>>),
    /// Layout and audio of playfield `id`, sent just before its [`Bg::World`]: `Report::dungeon` (`N3Msg_IsDungeon`) and the playfield's
    /// district music / ambience / emitters (`None` without sound data), handed to `Audio::set_playfield` when the world appears.
    Info(u32, bool, Option<Box<ao_audio::PlayfieldAudio>>),
    /// The character-creation world (`charactercreation_*.abiff` + connectors), decoded in the background.
    CcWorld(Result<Box<CcWorld>, String>),
}

#[derive(PartialEq, Clone, Copy)]
enum Screen {
    /// `LoginModule_c` state 0.
    Login,
    /// State 1 / 4: `ProgressWindow_c`.
    Progress { timeout: f32, joining: bool },
    /// LoginModule state 2 / LimboWindow_c.
    Limbo { timeout: f32, character_id: u32 },
    /// State 3.
    CharSelect,
    /// `CharCreateModule_t` (docs/screens.md §12).
    Create,
    /// Loading screen (`ServerLogin3DModule_t`): fading in or holding until the world is ready.
    Loading,
    InWorld,
}

enum Fade {
    In(f32),
    Hold,
    Out(f32),
}

struct Row {
    handle: ao_gui::ViewHandle,
    activated: bool,
}

struct Play {
    dir: PathBuf,
    /// Window-level Enhanced option (audit item 2): 0 Auto, 1/2/3 fixed GUI pixel scale; read once here (account-wide
    /// root `Prefs.xml`, like `texture_resolution` below) so `ao_render::Frontend::interface_scale` can hand it to the
    /// viewer before the OS window exists (the login screen must honour it too, not just after the HUD loads).
    interface_scale: u8,
    /// Account-wide Enhanced text preference, also applied before a HUD exists.
    smooth_text: bool,
    /// Texture colour depth is read once by native DisplaySystem construction.
    texture_resolution: i32,
    texture_quality: i32,
    texture_compressed: bool,
    ground_texture_quality: i32,
    ground_texture_patches: Vec<(usize, [i32; 2], usize)>,
    ground_texture_serial: u64,
    ground_texture_installed_quality: i32,
    gui: Gui,
    text: TextDb,
    pf_names: HashMap<u32, String>,
    audio: Option<Audio>,
    prefs: Prefs,
    tx: Sender<Bg>,
    rx: Receiver<Bg>,
    size: (u32, u32),
    screen: Screen,
    // servers
    servers: Option<Result<Vec<ServerEntry>, String>>,
    servers_fetching: bool,
    server_arg: Option<String>,
    // windows
    login_w: Option<WindowId>,
    progress_w: Option<WindowId>,
    char_w: Option<WindowId>,
    dialog_w: Option<(WindowId, DialogKind)>,
    progress_t: f32,
    session: Option<LoginSession>,
    /// bumped by `show_login`; tags the connect thread so a late result after Cancel/timeout is dropped
    conn_gen: u32,
    /// error pages `show_error` opened (asserted by the tests, which never launch a browser)
    opened_urls: Vec<String>,
    // character selection
    backdrop: Option<Box<Scene>>,
    chars: Vec<CharacterEntry>,
    rows: Vec<Row>,
    slots: i32,
    selected: Option<usize>,
    worker: Option<preview::Worker>,
    mesh_base: usize,
    clips: HashMap<Option<usize>, Vec<Scene>>,
    playing: Option<(Option<usize>, f32)>,
    char_pos: [f32; 3],
    char_ready: bool,
    rng: u32,
    preview_error: Option<String>,
    // loading screen / world
    loading_img: Option<(GfxId, u32, u32)>,
    fade: Fade,
    /// Loading module remains active through world installation until Alive closes it.
    loading_active: bool,
    world_ready: bool,
    /// `FlowControlModule_t::m_isTeleporting` [GUI 0x102635d8] while in the world: set by `TeleportStarted`, cleared by `TeleportEnded`
    /// (the new world appeared). The HUD and chat stay; the 3D world is hidden (docs/zone/world.md §10.2).
    teleporting: bool,
    /// `InputConfig_t+0x18` (`isUserInputStopped`) and `DisplaySystem+0x44 = 0` (3D viewport off): both set by `TeleportStartedMessage`, both
    /// cleared by `AliveMessage`, i.e. by the server's echo of our `CharInPlay` (docs/zone/world.md §10.2).
    awaiting_alive: bool,
    /// `Report::dungeon` of the current playfield (`N3Msg_IsDungeon`).
    dungeon: bool,
    /// `AOMAC_ZONE_TIMING=1`: log zone-change latency stages to stderr (recv/world_ready/first_frame/char_in_play_sent/server_alive/dynels_visible),
    /// each as milliseconds since the stage-starting message. A measurement instrument only; off by default.
    zone_timing: bool,
    /// Start of the current `zone_timing` cycle, time-boxed (see `Play::zt_begin`): a new `recv_*` more than 3 s after this one starts a fresh cycle.
    zone_timer: Option<std::time::Instant>,
    /// One-shot guard for the `dynels_visible` stage within the current `zone_timing` cycle.
    zone_timing_dynels_seen: bool,
    /// District music / ambience of the loading playfield (`Bg::Info`), switched in when the world appears.
    world_audio: Option<(u32, Box<ao_audio::PlayfieldAudio>)>,
    /// State of the current zone connection (`ZoneHandoff` .. disconnect).
    zone: zone::Zone,
    /// Signed `WaitingToStartGame` counters; either must become strictly negative.
    world_frames: i32,
    world_seconds: f32,
    fake: bool,
    /// `--fake-charlist`: the replies an in-process fake login server would send (random name, created, hand-off, …).
    fake_events: std::collections::VecDeque<LoginEvent>,
    /// `--fake-charlist`: character index to show first once the window size is known.
    pending_fake: Option<usize>,
    pending_user: String,
    world_scene: Option<Box<Scene>>,
    /// Ground image of the loading playfield for the Map window, handed to the HUD with the world.
    world_ground: Option<(u32, Box<WorldGround>)>,
    /// Live sky of the loading/loaded playfield, installed in the viewer when the world appears.
    world_sky: Option<ao_formats::playfield::SkyClock>,
    world_weather: Option<ao_formats::weather::Weather>,
    /// Native storm schedules + fan render/audio state, reset on zone load (`start_world_load`).
    world_lightning: lightning::LightningFx,
    world_life: Option<(native_life_spawn::LifeWorld, native_life_render::LifeModels)>,
    wildlife_enabled: bool,
    life_characters: Vec<native_life::CharacterSensor>,
    life_dead: Vec<glam::Vec3>,
    life_dangers: Vec<native_life::CharacterSensor>,
    /// GC environment-manager +0xc oscillator phase; retained across playfield changes.
    effect_environment_phase: f32,
    time: f32,
    // character creation / deletion
    /// The delete window kept open below its `MatchError` box (the original's `DialogBox_c::Go` is modal on top of it).
    under_dialog: Option<WindowId>,
    cc: Option<Box<create::Create>>,
    cc_world: Option<Box<CcWorld>>,
    char_list: CharacterList,
    /// `SetLoadingScreen(n)`: the next loading screen is `welcome_to_rubika.jpg`.
    welcome_image: bool,
    loading_name: &'static str,
    /// The in-world interface (`ControlCenterModule_c`), created when the world appears.
    hud: Option<hud::Hud>,
    /// Zone frames for the HUD (`Hud::wants_zone_frame`) that arrived before it existed (the own perk map comes with the world load).
    hud_pending: Vec<ao_net::frame::Frame>,
    /// Chat hub (`ChatGUIModule_c`), created at the zone hand-off; the windows open with the world.
    chat: Option<chat::Chat>,
    /// The own character in the world (movement, avatar, camera, controls); `None` outside the world.
    player: Option<player::Player>,
    /// Combat / action layer of the zone connection (`combat/module.rs`).
    fight: Option<combat::module::Module>,
    /// Processed own events: attack note, SpecialAttack, equipment, item use, level (live harness only).
    #[cfg(test)]
    live_attack_events: [u64; 5],
    /// Seconds since a successful `N3Msg_StartCamping` (the logout countdown, `hud_use.rs`).
    camp: Option<f32>,
    /// The "Logout" timer bar of the camp countdown (`FlowControlModule_t::m_pcCampTimer`, `hud_use.rs`).
    camp_bar: Option<ao_gui::WindowId>,
    /// `/camp` / `/quit` state (`m_eLoggingOutTimed`, `m_nQuitToSystemTime`; logout.rs).
    logout: logout::Logout,
    /// NPC dialogue / object use (`interact.rs`), created at the zone hand-off.
    interact: Option<interact::Interact>,
    /// Account name and password of the login, for the chat-server login only (the original keeps `cPlayerName`/`cPlayerPasswd`).
    login_cred: Option<(String, String)>,
}

#[derive(Clone, Copy, PartialEq)]
enum DialogKind {
    /// Error text; Cancel button relabelled "OK".
    Message,
    /// Native TextException fatal notice; dismissing finishes shutdown.
    Fatal,
    Activate,
    /// `CharCreateModule_t::AskExitMessage` ("ExitCC").
    ExitCc,
    /// `CharDeleteWindow_c` (name confirmation).
    Delete,
}

impl Play {
    /// Everything `run` needs before the background threads start; also the headless entry point of the tests.
    fn new(dir: PathBuf, fake_charlist: Option<usize>, server_arg: Option<String>, audio: Option<Audio>) -> Result<Self> {
        let text = TextDb::load(&dir)?;
        let labels = TextDb::load(&dir)?;
        let gui = Gui::new(
            &dir,
            Some(Box::new(move |s: &str| {
                let r = labels.label(s);
                (r != s).then_some(r)
            })),
        )?;
        let (tx, rx) = channel();
        let mut texture_values = dvalue::DValues::new(&dir);
        if let Some(root) = prefs::dir() {
            if let Ok(xml) = std::fs::read_to_string(root.join("Prefs.xml")) {
                texture_values.load_config(&xml, dvalue::CAT_MAIN, false);
            }
        }
        Ok(Play {
            texture_resolution: texture_values.get_i64("DisplayTextureResolution").unwrap_or(2) as i32,
            texture_quality: texture_values.get_i64("DisplayOtherTextureQuality").unwrap_or(3) as i32,
            texture_compressed: texture_values.flag("DisplayCompressedTextures"),
            wildlife_enabled: texture_values.flag("Wildlife"),
            interface_scale: texture_values.get_i64("AomacInterfaceScale").unwrap_or(0).clamp(0, 3) as u8,
            smooth_text: texture_values.flag("AomacSmoothText"),
            ground_texture_quality: texture_values.get_i64("GroundTextureQuality").unwrap_or(2) as i32,
            ground_texture_patches: Vec::new(),
            ground_texture_serial: 0,
            ground_texture_installed_quality: texture_values.get_i64("GroundTextureQuality").unwrap_or(2) as i32,
            pf_names: screens::pfnr_names(&dir).unwrap_or_default(),
            audio,
            prefs: Prefs::load(),
            tx,
            rx,
            size: (0, 0),
            screen: Screen::Login,
            servers: None,
            servers_fetching: false,
            server_arg,
            login_w: None,
            progress_w: None,
            char_w: None,
            dialog_w: None,
            progress_t: 0.0,
            session: None,
            conn_gen: 0,
            opened_urls: vec![],
            backdrop: None,
            chars: vec![],
            rows: vec![],
            slots: 0,
            selected: None,
            worker: None,
            mesh_base: 0,
            clips: HashMap::new(),
            playing: None,
            char_pos: [0.0; 3],
            char_ready: false,
            rng: 0x9E37_79B9,
            preview_error: None,
            loading_img: None,
            fade: Fade::In(0.0),
            loading_active: false,
            world_ready: false,
            teleporting: false,
            awaiting_alive: false,
            dungeon: false,
            zone_timing: std::env::var_os("AOMAC_ZONE_TIMING").is_some(),
            zone_timer: None,
            zone_timing_dynels_seen: false,
            world_audio: None,
            zone: zone::Zone::default(),
            world_frames: IN_PLAY_FRAMES,
            world_seconds: IN_PLAY_SECONDS,
            fake: fake_charlist.is_some(),
            fake_events: Default::default(),
            under_dialog: None,
            pending_fake: fake_charlist,
            pending_user: String::new(),
            world_scene: None,
            world_ground: None,
            world_sky: None,
            world_weather: None,
            world_lightning: lightning::LightningFx::default(),
            world_life: None,
            life_characters: Vec::new(),
            life_dead: Vec::new(),
            life_dangers: Vec::new(),
            effect_environment_phase: 0.0,
            time: 0.0,
            cc: None,
            cc_world: None,
            char_list: CharacterList::default(),
            welcome_image: false,
            loading_name: "",
            hud: None,
            hud_pending: vec![],
            chat: None,
            player: None,
            fight: None,
            #[cfg(test)]
            live_attack_events: [0; 5],
            camp: None,
            camp_bar: None,
            logout: Default::default(),
            interact: None,
            login_cred: None,
            text,
            gui,
            dir,
        })
    }

    fn start_backdrop(&self) {
        let (tx, dir) = (self.tx.clone(), self.dir.clone());
        std::thread::spawn(move || {
            let r = open_texture_store(&dir).and_then(|s| screens::login_world_scene(&s, 0)).map(Box::new).map_err(|e| format!("{e:#}"));
            let _ = tx.send(Bg::Backdrop(r));
        });
    }
}

/// Loads the native Main-category texture family before a background asset loader starts.
fn open_texture_store(dir: &std::path::Path) -> anyhow::Result<RecordStore> {
    let store = RecordStore::open(dir)?;
    let mut values = dvalue::DValues::new(dir);
    if let Some(root) = prefs::dir() {
        if let Ok(xml) = std::fs::read_to_string(root.join("Prefs.xml")) {
            values.load_config(&xml, dvalue::CAT_MAIN, false);
        }
    }
    store.set_other_texture_quality(values.get_i64("DisplayOtherTextureQuality").unwrap_or(3) as i32);
    store.set_ground_texture_quality(values.get_i64("GroundTextureQuality").unwrap_or(2) as i32);
    Ok(store)
}

pub fn run(dir: PathBuf, fake_charlist: Option<usize>, server_arg: Option<String>) -> Result<()> {
    let audio = Audio::start(&dir).map_err(|e| eprintln!("audio disabled: {e:#}")).ok().inspect(|a| {
        if std::env::var_os("AOMAC_AUDIO_MUTE").is_some() {
            a.set_output_gain(0.0);
        }
    });
    let mut p = Play::new(dir.clone(), fake_charlist, server_arg, audio)?;
    p.start_backdrop();
    if fake_charlist.is_none() {
        p.request_servers();
    }
    ao_render::run_frontend(Scene::default(), p)
}
