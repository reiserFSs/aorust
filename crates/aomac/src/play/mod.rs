//! `aomac play`: login screen -> character select (3D preview) -> zone hand-off -> world.
//! egui on top of the wgpu renderer (`ao_render::run_frontend`); network runs on `ao_net::client`'s threads.

mod preview;
mod prefs;

use ao_audio::Audio;
use ao_net::client::{fetch_servers, LoginEvent, LoginSession, ServerEntry};
use ao_net::msg::{CharacterEntry, CharacterList};
use ao_rdb::RecordStore;
use ao_render::egui::{self, Align2, Color32, RichText, Vec2};
use ao_render::{Camera, Frontend, Host, Vec3};
use ao_scene::{Scene, Texture};
use anyhow::Result;
use prefs::Prefs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Instant;

const ACCENT: Color32 = Color32::from_rgb(150, 110, 245);
/// Playfield identity type of `PlayfieldProxy::playfield` (`IdentityType.Playfield`, protocol.md §6): `instance` is the playfield id.
const PLAYFIELD_IDENTITY: i32 = 0xC79D;
const CLICK: &str = "sfx/gui/if_01"; // guess: the original's button sound table was not located

/// Messages from worker threads.
enum Bg {
    Servers(Result<Vec<ServerEntry>, String>),
    Connected(Result<LoginSession, String>),
    Preview(u64, preview::Msg),
    World(u32, Result<Scene, String>),
}

#[derive(PartialEq)]
enum Screen {
    Login,
    CharSelect,
    /// `select_character` sent; waiting for ZoneHandoff / ZoneConnected.
    EnteringZone,
    /// Zone connected; playfield loading.
    LoadingWorld,
    InWorld,
}

struct Play {
    dir: PathBuf,
    audio: Option<Audio>,
    prefs: Prefs,
    tx: Sender<Bg>,
    rx: Receiver<Bg>,
    screen: Screen,
    styled: bool,
    start: Instant,
    // login
    servers: Option<Result<Vec<ServerEntry>, String>>,
    server: usize,
    password: String,
    busy: bool,
    status: String,
    error: bool,
    session: Option<LoginSession>,
    // character select
    chars: Vec<CharacterEntry>,
    selected: usize,
    gen: u64,
    stage: Option<preview::Stage>,
    preview_error: Option<String>,
    angle: f32,
    login_bg: Option<egui::TextureHandle>,
    backdrop: Option<Texture>,
    // world
    zone_msgs: usize,
    fake: bool,
}

pub fn run(dir: PathBuf, fake_charlist: Option<usize>) -> Result<()> {
    let (tx, rx) = channel();
    let audio = Audio::start(&dir).map_err(|e| eprintln!("audio disabled: {e:#}")).ok();
    let mut p = Play {
        audio,
        prefs: Prefs::load(),
        tx,
        rx,
        screen: Screen::Login,
        styled: false,
        start: Instant::now(),
        servers: None,
        server: 0,
        password: String::new(),
        busy: false,
        status: String::new(),
        error: false,
        session: None,
        chars: vec![],
        selected: 0,
        gen: 0,
        stage: None,
        preview_error: None,
        angle: 0.0,
        login_bg: None,
        backdrop: load_image(&dir.join("cd_image/gui/Default/gfx/welcome_to_rubika.jpg")),
        zone_msgs: 0,
        fake: fake_charlist.is_some(),
        dir,
    };
    if let Some(i) = fake_charlist {
        p.show_characters(fake_list(), i);
    } else {
        p.refresh_servers();
    }
    ao_render::run_frontend(Scene::default(), p)
}

fn load_image(path: &Path) -> Option<Texture> {
    let img = image::open(path).map_err(|e| eprintln!("{}: {e}", path.display())).ok()?.to_rgba8();
    Some(Texture { width: img.width(), height: img.height(), rgba: img.into_raw() })
}

fn profession(id: i32) -> &'static str {
    // CellAO `Profession` enum -- [inference], not read from the client.
    match id {
        1 => "Soldier",
        2 => "Martial Artist",
        3 => "Engineer",
        4 => "Fixer",
        5 => "Agent",
        6 => "Adventurer",
        7 => "Trader",
        8 => "Bureaucrat",
        9 => "Enforcer",
        10 => "Doctor",
        11 => "Nano-Technician",
        12 => "Meta-Physicist",
        14 => "Keeper",
        15 => "Shade",
        _ => "?",
    }
}

fn breed(id: i32) -> &'static str {
    match id {
        1 => "Solitus",
        2 => "Opifex",
        3 => "Nanomage",
        4 => "Atrox",
        _ => "?",
    }
}

/// Offline `--fake-charlist` data (one of every breed).
fn fake_list() -> CharacterList {
    let mk = |id, name: &str, breed, gender, profession, level, head| {
        let mut e = CharacterEntry { id, created: true, ..Default::default() };
        e.proxy.playfield.kind = PLAYFIELD_IDENTITY;
        e.proxy.playfield.instance = 4001;
        e.info = ao_net::msg::CharacterInfo { id, name: name.into(), breed, gender, profession, level, area: "Rubi-Ka".into(), head, ..Default::default() };
        e
    };
    CharacterList {
        characters: vec![
            mk(1, "Atroxfighter", 4, 1, 9, 120, 0),
            mk(2, "Solitusdoc", 1, 2, 10, 85, 1),
            mk(3, "Nanomagey", 3, 2, 12, 40, 0),
            mk(4, "Opifexfixer", 2, 1, 4, 205, 0),
        ],
        allowed_characters: 6,
        ..Default::default()
    }
}

impl Play {
    fn click(&self) {
        if let Some(a) = &self.audio {
            a.play_sfx(CLICK, 0.5);
        }
    }

    fn set_status(&mut self, s: impl Into<String>, error: bool) {
        self.status = s.into();
        self.error = error;
    }

    fn refresh_servers(&mut self) {
        self.servers = None;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Bg::Servers(fetch_servers().map_err(|e| format!("{e:#}"))));
        });
    }

    fn login(&mut self, server: ServerEntry) {
        self.prefs.server = server.name.clone();
        self.prefs.save();
        self.busy = true;
        self.set_status(format!("Connecting to {} ({}:{})...", server.name, server.ip, server.port), false);
        let (user, pass, tx) = (self.prefs.username.clone(), std::mem::take(&mut self.password), self.tx.clone());
        // Connect blocks (<= 10 s); the password only lives in this closure until handed to the session thread.
        std::thread::spawn(move || {
            let r = LoginSession::connect(&server).map(|s| {
                s.login(&user, &pass);
                s
            });
            let _ = tx.send(Bg::Connected(r.map_err(|e| format!("{e:#}"))));
        });
    }

    fn show_characters(&mut self, list: CharacterList, first: usize) {
        self.chars = list.characters;
        self.screen = Screen::CharSelect;
        self.busy = false;
        self.set_status(format!("{} of {} character slots used", self.chars.len(), list.allowed_characters), false);
        self.select(first.min(self.chars.len().saturating_sub(1)));
    }

    /// Starts building the 3D preview of character `i`.
    fn select(&mut self, i: usize) {
        self.selected = i;
        self.gen += 1;
        self.preview_error = None;
        if let Some(c) = self.chars.get(i) {
            let (dir, info, tx, gen) = (self.dir.clone(), c.info.clone(), self.tx.clone(), self.gen);
            let (ptx, prx) = channel();
            std::thread::spawn(move || preview::build(&dir, info, ptx, gen));
            // Forward preview messages into the main channel (keeps one receiver in the UI).
            std::thread::spawn(move || {
                for (g, m) in prx {
                    if tx.send(Bg::Preview(g, m)).is_err() {
                        break;
                    }
                }
            });
        }
    }

    fn back_to_login(&mut self, msg: &str, error: bool, host: &mut Host) {
        self.session = None;
        self.busy = false;
        self.stage = None;
        self.screen = Screen::Login;
        host.set_scene(Scene::default());
        host.fly = false;
        self.set_status(msg, error);
    }

    fn enter_world(&mut self) {
        let Some(c) = self.chars.get(self.selected) else { return };
        self.click();
        if self.fake || self.session.is_none() {
            self.set_status("Offline preview (--fake-charlist): no server session to enter the world with", true);
            return;
        }
        let id = c.id as u32;
        if let Some(s) = &self.session {
            s.select_character(id);
        }
        self.screen = Screen::EnteringZone;
        self.set_status(format!("Entering the world as {}...", c.info.name), false);
    }

    /// M3 seam: called for every session event once the zone connection exists (and with the first zone
    /// frames' summary on `ZoneConnected`). Zone message decoding/handling plugs in here.
    fn on_zone_event(&mut self, ev: LoginEvent, host: &mut Host) {
        match ev {
            LoginEvent::Disconnected(why) => self.set_status(format!("Zone connection closed: {why}"), true),
            LoginEvent::Status(s) => self.set_status(s, false),
            other => eprintln!("zone event (unhandled until M3): {other:?}"),
        }
        let _ = host;
    }

    fn pump(&mut self, host: &mut Host) {
        while let Ok(bg) = self.rx.try_recv() {
            match bg {
                Bg::Servers(r) => {
                    if let Ok(list) = &r {
                        self.server = list.iter().position(|s| s.name == self.prefs.server).unwrap_or(0);
                    }
                    self.servers = Some(r);
                }
                Bg::Connected(Ok(s)) => {
                    self.session = Some(s);
                    self.set_status("Connected, authenticating...", false);
                }
                Bg::Connected(Err(e)) => {
                    self.busy = false;
                    self.set_status(format!("Connection failed: {e}"), true);
                }
                Bg::Preview(gen, m) if gen == self.gen => self.on_preview(m, host),
                Bg::Preview(..) => {}
                Bg::World(id, r) => self.on_world(id, r, host),
            }
        }
        while let Some(ev) = self.session.as_ref().and_then(|s| s.poll()) {
            match (&self.screen, ev) {
                (Screen::Login, LoginEvent::Status(s)) => self.set_status(s, false),
                (Screen::Login, LoginEvent::CharacterList(l)) => self.show_characters(l, 0),
                (_, LoginEvent::LoginError { code, message }) => {
                    let text = if message.is_empty() { ao_net::client::login_error_text(code) } else { message };
                    self.back_to_login(&format!("Login failed: {text}"), true, host);
                }
                (Screen::CharSelect | Screen::EnteringZone, LoginEvent::Status(s)) => self.set_status(s, false),
                (Screen::EnteringZone, LoginEvent::ZoneHandoff { zone_ip, zone_port, .. }) => {
                    self.set_status(format!("Connecting to zone {zone_ip}:{zone_port}..."), false)
                }
                (Screen::EnteringZone, LoginEvent::ZoneConnected { messages }) => {
                    self.zone_msgs = messages.len();
                    self.start_world_load();
                }
                (Screen::Login | Screen::CharSelect | Screen::EnteringZone, LoginEvent::Disconnected(why)) => {
                    self.back_to_login(&format!("Disconnected: {why}"), true, host)
                }
                (_, ev) => self.on_zone_event(ev, host),
            }
        }
    }

    fn on_preview(&mut self, m: preview::Msg, host: &mut Host) {
        match m {
            preview::Msg::First(scene) => {
                let (stage, up, eye, at) = preview::Stage::new(*scene, self.backdrop.as_ref());
                host.set_scene(up);
                host.camera = Camera::look_at(eye, at);
                self.stage = Some(stage);
                self.angle = 0.0;
            }
            preview::Msg::Frame(s) => {
                if let Some(st) = &mut self.stage {
                    st.push(*s);
                }
            }
            preview::Msg::Failed(e) => {
                eprintln!("preview: {e}");
                self.preview_error = Some(e);
            }
        }
    }

    fn start_world_load(&mut self) {
        self.screen = Screen::LoadingWorld;
        let Some(c) = self.chars.get(self.selected) else { return };
        let pf = &c.proxy.playfield;
        if pf.kind != PLAYFIELD_IDENTITY {
            eprintln!("playfield identity type {:#x} (expected {PLAYFIELD_IDENTITY:#x}); using instance {} as the id", pf.kind, pf.instance);
        }
        let (id, dir, tx) = (pf.instance as u32, self.dir.clone(), self.tx.clone());
        self.set_status(format!("Zone connected ({} frames). Loading playfield {id}...", self.zone_msgs), false);
        std::thread::spawn(move || {
            let r = RecordStore::open(&dir)
                .and_then(|store| ao_formats::playfield::load_playfield_at(&store, &dir, id, ao_formats::playfield::DEFAULT_DAY_TIME))
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Bg::World(id, r));
        });
    }

    fn on_world(&mut self, id: u32, r: Result<Scene, String>, host: &mut Host) {
        match r {
            Ok(scene) => {
                let (eye, at) = match (scene.spawn, scene.spawn_look_at) {
                    (Some(e), Some(a)) => (Vec3::from(e), Vec3::from(a)),
                    (Some(e), None) => (Vec3::from(e), Vec3::from(e) + Vec3::Z),
                    _ => ao_render::default_view(&scene),
                };
                host.set_scene(scene);
                host.camera = Camera::look_at(eye, at);
                host.fly = true;
                self.screen = Screen::InWorld;
                self.stage = None;
                self.set_status(format!("In playfield {id}"), false);
            }
            Err(e) => {
                self.screen = Screen::CharSelect;
                self.set_status(format!("Zone connected, but playfield {id} failed to load: {e}"), true);
            }
        }
    }

    fn style(&mut self, ctx: &egui::Context) {
        self.styled = true;
        ctx.global_style_mut(|s| {
            for f in s.text_styles.values_mut() {
                f.size *= 1.2;
            }
            s.visuals = egui::Visuals::dark();
            s.visuals.selection.bg_fill = ACCENT.gamma_multiply(0.7);
            s.visuals.widgets.hovered.bg_fill = ACCENT.gamma_multiply(0.5);
            s.visuals.window_fill = Color32::from_rgba_unmultiplied(12, 10, 24, 225);
            s.visuals.panel_fill = Color32::from_rgba_unmultiplied(12, 10, 24, 200);
            s.visuals.window_stroke = egui::Stroke::new(1.0, ACCENT);
            s.spacing.item_spacing = Vec2::new(8.0, 8.0);
            s.spacing.button_padding = Vec2::new(14.0, 6.0);
        });
    }

    fn status_line(&self, ui: &mut egui::Ui) {
        if !self.status.is_empty() {
            let color = if self.error { Color32::from_rgb(255, 120, 120) } else { Color32::from_gray(200) };
            ui.add(egui::Label::new(RichText::new(&self.status).color(color)).wrap());
        }
    }

    fn login_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let screen = ctx.content_rect();
        let tex = self.login_bg.get_or_insert_with(|| {
            let t = load_image(&self.dir.join("cd_image/gui/Default/gfx/ai_loading_login.png")).unwrap_or(Texture { width: 1, height: 1, rgba: vec![8, 6, 16, 255] });
            ctx.load_texture("login_bg", egui::ColorImage::from_rgba_unmultiplied([t.width as usize, t.height as usize], &t.rgba), Default::default())
        });
        // "cover": scale the 4:3 art to fill the window, cropping the overflow.
        let ar = tex.size()[0] as f32 / tex.size()[1] as f32;
        let (uw, uh) = if screen.aspect_ratio() > ar { (1.0, ar / screen.aspect_ratio()) } else { (screen.aspect_ratio() / ar, 1.0) };
        let uv = egui::Rect::from_min_max(egui::pos2((1.0 - uw) / 2.0, (1.0 - uh) / 2.0), egui::pos2((1.0 + uw) / 2.0, (1.0 + uh) / 2.0));
        ui.painter().image(tex.id(), screen, uv, Color32::WHITE);

        let mut login: Option<ServerEntry> = None;
        egui::Window::new("login").title_bar(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, 150.0]).fixed_size([400.0, 0.0]).show(&ctx, |ui| {
            ui.heading(RichText::new("Project Rubi-Ka").color(ACCENT));
            ui.add_enabled_ui(!self.busy, |ui| {
                match &self.servers {
                    None => {
                        ui.label("Fetching server list...");
                    }
                    Some(Err(e)) => {
                        ui.colored_label(Color32::from_rgb(255, 120, 120), format!("Server list unavailable: {e}"));
                        if ui.button("Retry").clicked() {
                            self.refresh_servers();
                        }
                    }
                    Some(Ok(list)) if list.is_empty() => {
                        ui.label("No servers online.");
                    }
                    Some(Ok(list)) => {
                        for (i, s) in list.iter().enumerate() {
                            ui.selectable_value(&mut self.server, i, format!("{}  -  {} online", s.name, s.players));
                        }
                    }
                }
                egui::Grid::new("creds").num_columns(2).show(ui, |ui| {
                    ui.label("Username");
                    let user = ui.add(egui::TextEdit::singleline(&mut self.prefs.username).desired_width(240.0));
                    ui.end_row();
                    ui.label("Password");
                    let pass = ui.add(egui::TextEdit::singleline(&mut self.password).password(true).desired_width(240.0));
                    ui.end_row();
                    let enter = pass.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let _ = user;
                    ui.label("");
                    ui.checkbox(&mut self.prefs.remember, "Remember username");
                    ui.end_row();
                    let ready = !self.prefs.username.is_empty() && !self.password.is_empty();
                    if (ui.add_enabled(ready, egui::Button::new("Login")).clicked() || (enter && ready)) && !self.busy {
                        if let Some(Ok(list)) = &self.servers {
                            login = list.get(self.server).cloned();
                        }
                    }
                    if ui.button("Quit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            });
            self.status_line(ui);
        });
        if let Some(s) = login {
            self.click();
            self.login(s);
        }
    }

    fn char_ui(&mut self, ui: &mut egui::Ui, host: &mut Host, dt: f32) {
        let mut pick = None;
        let (mut enter, mut back) = (false, false);
        egui::Panel::right("chars").exact_size(320.0).show(ui, |ui| {
            ui.heading(RichText::new("Character selection").color(ACCENT));
            egui::ScrollArea::vertical().max_height(ui.available_height() - 150.0).show(ui, |ui| {
                for (i, c) in self.chars.iter().enumerate() {
                    let i_ = &c.info;
                    let text = format!("{}\nLevel {} {} - {} {}\n{}", i_.name, i_.level, profession(i_.profession), breed(i_.breed), if i_.gender == 2 { "female" } else { "male" }, area(c));
                    if ui.add(egui::Button::selectable(self.selected == i, text).min_size(Vec2::new(ui.available_width(), 0.0))).clicked() && self.selected != i {
                        pick = Some(i);
                    }
                }
            });
            ui.separator();
            ui.label(format!("{} characters", self.chars.len()));
            ui.horizontal(|ui| {
                enter = ui.add_enabled(!self.chars.is_empty(), egui::Button::new("Enter world")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter));
                back = ui.button("Log out").clicked();
            });
            if let Some(e) = &self.preview_error {
                ui.colored_label(Color32::from_rgb(255, 180, 90), format!("3D preview unavailable: {e}"));
            }
            self.status_line(ui);
        });
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
            let r = ui.allocate_response(ui.available_size(), egui::Sense::drag());
            self.angle += r.drag_delta().x * 0.01;
            if r.hovered() {
                ui.set_cursor_icon(egui::CursorIcon::Grab);
            }
        });
        let t = self.start.elapsed().as_secs_f32();
        if let Some(st) = &mut self.stage {
            host.repose(st.pose(t, self.angle));
        }
        let _ = dt;
        if let Some(i) = pick {
            self.click();
            self.select(i);
        }
        if back {
            self.click();
            self.back_to_login("Logged out", false, host);
        } else if enter {
            self.enter_world();
        }
    }

    fn world_ui(&mut self, ctx: &egui::Context, host: &mut Host) {
        if let Some(a) = &self.audio {
            let c = host.camera;
            a.set_listener(c.pos.to_array(), c.forward().to_array(), [0.0, 1.0, 0.0]);
        }
        egui::Area::new("hud".into()).anchor(Align2::LEFT_TOP, [10.0, 10.0]).show(ctx, |ui| {
            ui.label(RichText::new(&self.status).color(Color32::WHITE).background_color(Color32::from_black_alpha(140)));
        });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            host.quit = true;
        }
    }
}

fn area(c: &CharacterEntry) -> String {
    if c.info.area.is_empty() { format!("Playfield {}", c.proxy.playfield.instance) } else { c.info.area.clone() }
}

impl Frontend for Play {
    fn frame(&mut self, ui: &mut egui::Ui, host: &mut Host, dt: f32) {
        if !self.styled {
            self.style(&ui.ctx().clone());
        }
        self.pump(host);
        match self.screen {
            Screen::Login => self.login_ui(ui),
            Screen::CharSelect => self.char_ui(ui, host, dt),
            Screen::EnteringZone | Screen::LoadingWorld => {
                let ctx = ui.ctx().clone();
                egui::Window::new("zone").title_bar(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(&ctx, |ui| {
                    ui.spinner();
                    self.status_line(ui);
                });
            }
            Screen::InWorld => self.world_ui(&ui.ctx().clone(), host),
        }
    }
}
