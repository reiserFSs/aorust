//! `aomac play`: the original login flow (docs/screens.md) -- login window, progress dialog, character selection with the
//! 3D `LoginWorld_c` backdrop and `CharacterViewer_c` preview, loading screen, then the zone's playfield.
//! The GUI is `ao_gui` (the client's own view XML + skin); the network is `ao_net::client`.

mod create;
mod delete;
mod flow;
mod prefs;
mod preview;

use anyhow::Result;
use ao_audio::Audio;
use ao_formats::screens::{self, TextDb};
use ao_formats::create::CcWorld;
use ao_gui::{DrawCmd, DrawList, Event, FontId, Gui, GfxId, InputEvent, Key, MouseButton, WindowId, WindowSize};
use ao_net::client::{fetch_servers, LoginEvent, LoginSession, ServerEntry};
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

enum Bg {
    Servers(Result<Vec<ServerEntry>, String>),
    Connected(u32, Result<LoginSession, String>),
    Backdrop(Result<Box<Scene>, String>),
    World(u32, Result<Box<Scene>, String>),
    /// The character-creation world (`charactercreation_*.abiff` + connectors), decoded in the background.
    CcWorld(Result<Box<CcWorld>, String>),
}

#[derive(PartialEq, Clone, Copy)]
enum Screen {
    /// `LoginModule_c` state 0.
    Login,
    /// State 1 / 4: `ProgressWindow_c`.
    Progress { timeout: f32, joining: bool },
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
    handle: usize,
    activated: bool,
}

struct Play {
    dir: PathBuf,
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
    world_ready: bool,
    zone_summary: usize,
    in_world_msg: String,
    fake: bool,
    /// `--fake-charlist`: the replies an in-process fake login server would send (random name, created, hand-off, …).
    fake_events: std::collections::VecDeque<LoginEvent>,
    /// `--fake-charlist`: character index to show first once the window size is known.
    pending_fake: Option<usize>,
    pending_user: String,
    world_scene: Option<Box<Scene>>,
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
}

#[derive(Clone, Copy, PartialEq)]
enum DialogKind {
    /// Error text; Cancel button relabelled "OK".
    Message,
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
        Ok(Play {
            pf_names: screens::pfnr_names(&dir).unwrap_or_default(),
            audio,
            prefs: Prefs::load(),
            tx,
            rx,
            size: (0, 0),
            screen: Screen::Login,
            servers: None,
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
            world_ready: false,
            zone_summary: 0,
            in_world_msg: String::new(),
            fake: fake_charlist.is_some(),
            fake_events: Default::default(),
            under_dialog: None,
            pending_fake: fake_charlist,
            pending_user: String::new(),
            world_scene: None,
            time: 0.0,
            cc: None,
            cc_world: None,
            char_list: CharacterList::default(),
            welcome_image: false,
            loading_name: "",
            text,
            gui,
            dir,
        })
    }
}

pub fn run(dir: PathBuf, fake_charlist: Option<usize>, server_arg: Option<String>) -> Result<()> {
    let audio = Audio::start(&dir).map_err(|e| eprintln!("audio disabled: {e:#}")).ok();
    let p = Play::new(dir.clone(), fake_charlist, server_arg, audio)?;
    {
        let (tx, dir) = (p.tx.clone(), dir);
        std::thread::spawn(move || {
            let r = RecordStore::open(&dir).and_then(|s| screens::login_world_scene(&s, 0)).map(Box::new).map_err(|e| format!("{e:#}"));
            let _ = tx.send(Bg::Backdrop(r));
        });
    }
    if fake_charlist.is_none() {
        let tx = p.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Bg::Servers(fetch_servers().map_err(|e| format!("{e:#}"))));
        });
    }
    ao_render::run_frontend(Scene::default(), p)
}
