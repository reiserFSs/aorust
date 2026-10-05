//! `aomac play`: the original login flow (docs/screens.md) -- login window, progress dialog, character selection with the
//! 3D `LoginWorld_c` backdrop and `CharacterViewer_c` preview, loading screen, then the zone's playfield.
//! The GUI is `ao_gui` (the client's own view XML + skin); the network is `ao_net::client`.

mod flow;
mod prefs;
mod preview;

use anyhow::Result;
use ao_audio::Audio;
use ao_formats::screens::{self, TextDb};
use ao_gui::{DrawCmd, DrawList, Event, FontId, Gui, GfxId, InputEvent, Key, WindowId, WindowSize};
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
    Connected(Result<LoginSession, String>),
    Backdrop(Result<Box<Scene>, String>),
    World(u32, Result<Box<Scene>, String>),
}

#[derive(PartialEq, Clone, Copy)]
enum Screen {
    /// `LoginModule_c` state 0.
    Login,
    /// State 1 / 4: `ProgressWindow_c`.
    Progress { timeout: f32, joining: bool },
    /// State 3.
    CharSelect,
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
    /// `--fake-charlist`: character index to show first once the window size is known.
    pending_fake: Option<usize>,
    pending_user: String,
    world_scene: Option<Box<Scene>>,
    time: f32,
}

#[derive(Clone, Copy, PartialEq)]
enum DialogKind {
    /// Error text; Cancel button relabelled "OK".
    Message,
    Activate,
}

pub fn run(dir: PathBuf, fake_charlist: Option<usize>, server_arg: Option<String>) -> Result<()> {
    let text = TextDb::load(&dir)?;
    let labels = TextDb::load(&dir)?;
    let localize: Option<Box<dyn Fn(&str) -> Option<String>>> = Some(Box::new(move |s: &str| {
        let r = labels.label(s);
        (r != s).then_some(r)
    }));
    let gui = Gui::new(&dir, localize)?;
    let (tx, rx) = channel();
    let audio = Audio::start(&dir).map_err(|e| eprintln!("audio disabled: {e:#}")).ok();
    let mut p = Play {
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
        pending_fake: None,
        pending_user: String::new(),
        world_scene: None,
        time: 0.0,
        text,
        gui,
        dir: dir.clone(),
    };
    {
        let (tx, dir) = (p.tx.clone(), dir);
        std::thread::spawn(move || {
            let r = RecordStore::open(&dir).and_then(|s| screens::login_world_scene(&s, 0)).map(Box::new).map_err(|e| format!("{e:#}"));
            let _ = tx.send(Bg::Backdrop(r));
        });
    }
    if let Some(first) = fake_charlist {
        p.pending_fake = Some(first);
    } else {
        let tx = p.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Bg::Servers(fetch_servers().map_err(|e| format!("{e:#}"))));
        });
    }
    ao_render::run_frontend(Scene::default(), p)
}
