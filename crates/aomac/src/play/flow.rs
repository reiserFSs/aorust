//! The screens of docs/screens.md as behaviour on top of `ao_gui` windows.

use super::*;
use ao_formats::screens::{ao_to_render, LOGIN_CAMERA, LOGIN_STAGE_Y_STEP};
use ao_net::msg::CharacterInfo;

fn center(size: (u32, u32), outer: (u32, u32)) -> (i32, i32) {
    // `Window::MoveToCenter` [GUI 0x10154986]: floor(0.5 * screen - 0.5 * frame)
    (((size.0 as f32) * 0.5 - (outer.0 as f32) * 0.5).floor() as i32, ((size.1 as f32) * 0.5 - (outer.1 as f32) * 0.5).floor() as i32)
}

fn breed_name(b: i32) -> &'static str {
    // Gamecode.dll GetBreedStr (docs/screens.md §5.3)
    match b {
        1 => "Solitus",
        2 => "Opifex",
        3 => "Nanomage",
        4 => "Atrox",
        _ => "Unknown",
    }
}

fn sex_name(s: i32) -> &'static str {
    match s {
        2 => "Male",
        3 => "Female",
        _ => "Unknown",
    }
}

/// Offline `--fake-charlist` data: one of every breed.
fn fake_list() -> CharacterList {
    let mk = |id, name: &str, breed, gender, profession, level, activated: bool| {
        let mut e = CharacterEntry { id, created: true, status: activated as i32, ..Default::default() };
        e.proxy.playfield.kind = PLAYFIELD_IDENTITY;
        e.proxy.playfield.instance = 566;
        e.info = CharacterInfo { id, name: name.into(), breed, gender, profession, level, ..Default::default() };
        e
    };
    CharacterList {
        characters: vec![
            mk(1, "Atroxfighter", 4, 2, 9, 120, true),
            mk(2, "Solitusdoc", 1, 3, 10, 85, true),
            mk(3, "Nanomagey", 3, 3, 12, 40, false),
            mk(4, "Opifexfixer", 2, 2, 4, 205, true),
        ],
        allowed_characters: 6,
        ..Default::default()
    }
}

impl Play {
    // ---- backdrop ----------------------------------------------------------------------------------------------

    /// `LoginWorld_c::SetStage`: stage 0 for login/progress, 1 for character selection; camera fixed (docs/screens.md §2).
    fn show_backdrop(&mut self, stage: u32, host: &mut Host) {
        let Some(b) = &self.backdrop else { return };
        let mut s = (**b).clone();
        for i in &mut s.instances {
            i.transform[3][1] = stage as f32 * LOGIN_STAGE_Y_STEP;
        }
        self.mesh_base = s.meshes.len();
        host.camera = Camera::look_at(Vec3::from(s.spawn.unwrap_or_default()), Vec3::from(s.spawn_look_at.unwrap_or_default()));
        host.set_scene(s);
    }

    // ---- window helpers ----------------------------------------------------------------------------------------

    fn open_centered(&mut self, view: &str) -> Option<WindowId> {
        match self.gui.open_framed_window(view, (0, 0), WindowSize::Preferred) {
            Ok(w) => {
                self.recenter(w);
                Some(w)
            }
            Err(e) => {
                eprintln!("{view}: {e:#}");
                None
            }
        }
    }

    fn recenter(&mut self, w: WindowId) {
        let pos = center(self.size, self.gui.outer_size(w));
        self.gui.set_window_pos(w, pos);
    }

    fn close_all(&mut self) {
        for w in [self.login_w.take(), self.progress_w.take(), self.char_w.take(), self.dialog_w.take().map(|d| d.0)].into_iter().flatten() {
            self.gui.close_window(w);
        }
    }

    // ---- state 0: login ----------------------------------------------------------------------------------------

    /// `LoginModule_c::Show(0)` + `LoginWindow_c::Focus`.
    pub(super) fn show_login(&mut self, host: &mut Host) {
        self.close_all();
        self.session = None;
        self.screen = Screen::Login;
        self.show_backdrop(0, host);
        let Some(w) = self.open_centered("LoginWindow") else { return };
        self.login_w = Some(w);
        let g = &mut self.gui;
        g.set_visible(w, "steam_btn", false); // view_flags 256: hidden unless running under Steam
        g.set_feature_flags(w, "password", ao_gui::tvf::PASSWORD);
        g.combo_set_items(w, "username", self.prefs.accounts.clone());
        if let Some(a) = self.prefs.accounts.get(self.prefs.selected_account) {
            g.set_text(w, "username", a);
        }
        g.set_enabled(w, "login_btn", false);
        g.set_default_button(w, "login_btn");
        g.focus(w, "password");
        self.recenter(w);
    }

    fn login_text_changed(&mut self, w: WindowId, view: &str, text: &str) {
        match view {
            // SlotUsernameModified [GUI 0x10014fa5]: password cleared, Login disabled, 39 chars
            "username" => {
                if text.chars().count() > 39 {
                    self.gui.set_text(w, "username", &text.chars().take(39).collect::<String>());
                }
                self.gui.set_text(w, "password", "");
                self.gui.set_enabled(w, "login_btn", false);
            }
            // SlotPasswordModified [0x100150a1]: Login iff both non-empty, 67 chars
            "password" => {
                if text.chars().count() > 67 {
                    self.gui.set_text(w, "password", &text.chars().take(67).collect::<String>());
                }
                let both = !text.is_empty() && !self.gui.text(w, "username").is_empty();
                self.gui.set_enabled(w, "login_btn", both);
            }
            _ => {}
        }
    }

    fn server(&mut self) -> Result<ServerEntry, String> {
        let list = match self.servers.as_ref() {
            None => return Err("The server list has not arrived yet.".into()),
            Some(Err(e)) => return Err(format!("Server list unavailable: {e}")),
            Some(Ok(l)) => l,
        };
        let wanted = self.server_arg.clone().unwrap_or_else(|| self.prefs.server.clone());
        let s = list.iter().find(|s| s.name.eq_ignore_ascii_case(&wanted)).or(list.first()).cloned().ok_or("No servers online.")?;
        self.prefs.server = s.name.clone();
        Ok(s)
    }

    /// `SlotLoginAccount` [GUI 0x10011f0c]: `Show(1)` then connect to the login handler.
    fn do_login(&mut self, host: &mut Host) {
        let Some(w) = self.login_w else { return };
        let user: String = self.gui.text(w, "username").chars().take(39).collect();
        let pass: String = self.gui.text(w, "password").chars().take(67).collect();
        if user.is_empty() || pass.is_empty() {
            return;
        }
        let server = match self.server() {
            Ok(s) => s,
            Err(e) => return self.message_box(&e),
        };
        eprintln!("connecting to {} ({}:{}, {} online)", server.name, server.ip, server.port, server.players);
        self.pending_user = user.clone();
        self.gui.set_text(w, "password", ""); // ResetConnectionAndConfig clears name/password
        self.show_progress(CONNECT_TIMEOUT, false, host);
        let tx = self.tx.clone();
        // connect blocks (<= 10 s); the password only lives in this closure until the session thread owns it
        std::thread::spawn(move || {
            let r = LoginSession::connect(&server)
                .inspect(|s| s.login(&user, &pass))
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Bg::Connected(r));
        });
    }

    // ---- state 1 / 4: progress ---------------------------------------------------------------------------------

    fn show_progress(&mut self, timeout: f32, joining: bool, host: &mut Host) {
        for w in [self.login_w.take(), self.progress_w.take(), self.char_w.take()].into_iter().flatten() {
            self.gui.close_window(w);
        }
        self.screen = Screen::Progress { timeout, joining };
        self.progress_t = 0.0;
        if !joining {
            self.show_backdrop(0, host);
        }
        self.progress_w = self.open_centered("ProgressDialog");
    }

    /// Error / notice box. The original shows login errors in an embedded web page (docs/screens.md §3.6, UNRESOLVED);
    /// this reuses the ProgressDialog view with its button relabelled.
    fn message_box(&mut self, text: &str) {
        if let Some((w, _)) = self.dialog_w.take() {
            self.gui.close_window(w);
        }
        if let Some(w) = self.open_centered("ProgressDialog") {
            self.gui.set_text(w, "message", text);
            self.gui.set_visible(w, "progress_bar", false);
            self.gui.set_text(w, "cancel_btn", "OK");
            self.gui.resize_window(w, WindowSize::Preferred);
            self.recenter(w);
            self.dialog_w = Some((w, DialogKind::Message));
        }
    }

    // ---- state 3: character selection --------------------------------------------------------------------------

    fn is_activated(e: &CharacterEntry) -> bool {
        // bit 0 of the per-row flag word (docs/screens.md §5.3); the server's `status` value semantics are unverified
        e.status & 1 != 0
    }

    fn show_characters(&mut self, list: CharacterList, host: &mut Host) {
        for w in [self.login_w.take(), self.progress_w.take(), self.char_w.take()].into_iter().flatten() {
            self.gui.close_window(w);
        }
        self.screen = Screen::CharSelect;
        self.chars = list.characters;
        self.slots = list.allowed_characters;
        self.selected = None;
        self.rows.clear();
        self.worker = None;
        self.show_backdrop(1, host);
        let (w, h) = self.size;
        let Ok(win) = self.gui.open_window("CharacterSelectionWindow", (0, 0), WindowSize::Fixed(w, h)) else { return };
        self.char_w = Some(win);
        let g = &mut self.gui;
        for b in ["login_btn", "create_btn", "delete_btn"] {
            g.set_enabled(win, b, false);
        }
        g.set_layout_vertical(win, "characters_view", true);
        g.set_default_button(win, "login_btn");
        let active = self.chars.iter().filter(|c| Self::is_activated(c)).count() as i32;
        for c in &self.chars {
            let Ok(h) = g.add_view(win, "characters_view", "CharacterSelectionItem") else { continue };
            let i = &c.info;
            let prof = if i.profession == 0 || i.profession == 0xff {
                self.text.by_key(506, "NotChosenYet").unwrap_or_default()
            } else {
                self.text.by_id(2004, i.profession as u32).unwrap_or_else(|| "Unknown".into())
            };
            g.set_text_in(h, "name_btn", &i.name);
            g.set_text_in(h, "level", &i.level.to_string());
            g.set_text_in(h, "gender", sex_name(i.gender));
            g.set_text_in(h, "breed", breed_name(i.breed));
            g.set_text_in(h, "profession", &prof);
            g.set_text_in(h, "location", &screens::location_text(&self.pf_names, c.proxy.playfield.instance as u32));
            g.set_toggle_in(h, "name_btn", true, false);
            g.set_visible_in(h, "detailed_view", false);
            let activated = Self::is_activated(c);
            if activated {
                for v in ["status", "status_left", "status_right", "status_lbl"] {
                    g.remove_view_in(h, v);
                }
            } else {
                g.set_text_in(h, "status", "Inactive");
                g.set_color_in(h, "status", 0xEE4444);
            }
            self.rows.push(Row { handle: h, activated });
        }
        let fmt = self.text.by_key(10000, "AvailableCharSlots").unwrap_or_else(|| "%d/%d slots available".into());
        let slots = fmt.replacen("%d", &(self.slots - active).to_string(), 1).replacen("%d", &self.slots.to_string(), 1);
        g.set_text(win, "slots_available", &slots);
        // New Character: enabled iff activeCount < slotCount. Character creation is not implemented (no CreateCharacter
        // message in ao-net), so it stays disabled.
        g.relayout_window(win);
        if let Ok(i) = usize::try_from(self.prefs.selected_character) {
            if i < self.chars.len() {
                self.select_row(i, host);
            }
        }
    }

    /// `SlotCharClicked` [GUI 0x1000d29b].
    fn select_row(&mut self, i: usize, host: &mut Host) {
        let Some(win) = self.char_w else { return };
        for (k, r) in self.rows.iter().enumerate() {
            let on = k == i;
            self.gui.set_toggle_in(r.handle, "name_btn", true, on);
            self.gui.set_visible_in(r.handle, "summary_view", !on);
            self.gui.set_visible_in(r.handle, "detailed_view", on);
        }
        self.gui.relayout_window(win);
        self.gui.set_enabled(win, "login_btn", true);
        // Delete: needs the DeleteCharacter message (not in ao-net) -> stays disabled.
        self.selected = Some(i);
        self.prefs.selected_character = i as i32;
        self.prefs.save();
        // CharacterViewer_c::Update: hide, rebuild for (breed, sex), show when ready
        let (breed, sex) = (self.chars[i].info.breed, self.chars[i].info.gender);
        self.show_backdrop(1, host);
        self.clips.clear();
        self.playing = None;
        self.char_ready = false;
        self.preview_error = None;
        self.worker = Some(preview::Worker::start(self.dir.clone(), breed, sex));
        let Ok((b, _)) = screens::wire_breed_sex(breed, sex) else { return };
        let mut p = LOGIN_CAMERA.pos;
        for (v, o) in p.iter_mut().zip(screens::CHAR_VIEWER_OFFSET) {
            *v += o;
        }
        p[1] += screens::feet_offset_build1(b);
        self.char_pos = ao_to_render(p);
    }

    fn step_selection(&mut self, d: i32, host: &mut Host) {
        let n = self.chars.len() as i32;
        if n == 0 {
            return;
        }
        // GUI 0x1000dd15: no wrap; with nothing selected Up picks the last row, Down the first
        let next = match self.selected {
            None => if d < 0 { n - 1 } else { 0 },
            Some(i) => (i as i32 + d).clamp(0, n - 1),
        };
        if Some(next as usize) != self.selected {
            self.select_row(next as usize, host);
        }
    }

    /// Preview animation: `CCCharacter_t::RunFunction` (docs/screens.md §5.5).
    fn tick_preview(&mut self, dt: f32, host: &mut Host) {
        let Some(wk) = &self.worker else { return };
        while let Ok(m) = wk.rx.try_recv() {
            match m {
                preview::Out::First(mut ch) => {
                    let Some(b) = &self.backdrop else { continue };
                    let mut s = (**b).clone();
                    for i in &mut s.instances {
                        i.transform[3][1] = LOGIN_STAGE_Y_STEP;
                    }
                    let base = s.meshes.len();
                    self.mesh_base = base;
                    s.textures.extend(std::mem::take(&mut ch.textures));
                    s.meshes.extend(std::mem::take(&mut ch.meshes));
                    for mut i in ch.instances.drain(..) {
                        i.mesh += base;
                        for k in 0..3 {
                            i.transform[3][k] += self.char_pos[k];
                        }
                        s.instances.push(i);
                    }
                    host.set_scene(s);
                    self.char_ready = true;
                }
                preview::Out::Clip(c, f) => {
                    self.clips.insert(c, f);
                    if c.is_none() && self.playing.is_none() {
                        self.playing = Some((None, 0.0));
                    }
                }
                preview::Out::Failed(e) => {
                    eprintln!("preview: {e}");
                    self.preview_error = Some(e);
                }
            }
        }
        let Some((clip, t)) = self.playing else { return };
        let len = self.clips[&clip].len();
        let mut t = t + dt;
        let mut clip = clip;
        if (t * preview::FPS) as usize >= len {
            // clip finished: rand() % 115 < 23 -> random social, else idle
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 17;
            self.rng ^= self.rng << 5;
            clip = None;
            if let Some(i) = screens::next_social(self.rng) {
                if self.clips.contains_key(&Some(i)) {
                    clip = Some(i);
                } else if let Some(w) = &self.worker {
                    w.request(Some(i)); // built in the background; idle plays meanwhile
                }
            }
            t = 0.0;
        }
        self.playing = Some((clip, t));
        let frames = &self.clips[&clip];
        let f = &frames[((t * preview::FPS) as usize).min(frames.len() - 1)];
        let mut meshes = vec![Mesh::default(); self.mesh_base];
        meshes.extend(f.meshes.iter().cloned());
        let instances = f
            .instances
            .iter()
            .map(|i| {
                let mut i = *i;
                i.mesh += self.mesh_base;
                for k in 0..3 {
                    i.transform[3][k] += self.char_pos[k];
                }
                i
            })
            .collect();
        host.repose(Scene { meshes, instances, ..Default::default() });
    }

    /// `SlotLoginPressed` [GUI 0x1000db4e].
    fn play_pressed(&mut self, host: &mut Host) {
        let Some(i) = self.selected else { return };
        let active = self.rows.iter().filter(|r| r.activated).count() as i32;
        if self.rows[i].activated || self.fake {
            return self.enter_world(host);
        }
        if active >= self.slots {
            let t = self.text.by_key(10000, "CharacterSlotsExhausted").unwrap_or_else(|| "There is no available character slot to activate this character.".into());
            return self.message_box(&t);
        }
        if let Some(w) = self.open_centered("CharacterActivateWindow") {
            let t = self.text.by_key(10000, "ActivateCharacterWarning").unwrap_or_default().replace("%s", &self.chars[i].info.name);
            self.gui.set_text(w, "confirmation_text", &t);
            self.gui.resize_window(w, WindowSize::Preferred);
            self.recenter(w);
            self.dialog_w = Some((w, DialogKind::Activate));
        }
    }

    /// `SlotSelectCharacter` [GUI 0x100119f7]: `s_nCharID = id; LoginCharacter(); Show(4)`.
    fn enter_world(&mut self, host: &mut Host) {
        let Some(i) = self.selected else { return };
        if self.fake {
            // debug only (--fake-charlist): no server session, show the loading screen
            self.worker = None;
            return self.start_loading(host);
        }
        if let Some(s) = &self.session {
            s.select_character(self.chars[i].id as u32);
        }
        self.show_progress(JOIN_TIMEOUT, true, host);
    }

    // ---- loading screen ----------------------------------------------------------------------------------------

    /// `AFCM::AddProgram(5)`: `ServerLogin3DModule_t` (docs/screens.md §7).
    fn start_loading(&mut self, host: &mut Host) {
        for w in [self.login_w.take(), self.progress_w.take(), self.char_w.take(), self.dialog_w.take().map(|d| d.0)].into_iter().flatten() {
            self.gui.close_window(w);
        }
        self.screen = Screen::Loading;
        self.fade = Fade::In(0.0);
        self.world_ready = false;
        self.world_scene = None;
        host.fly = false;
        if self.loading_img.is_none() {
            let path = self.dir.join("cd_image/gui/Default/gfx/ai_loading_login.png");
            match image::open(&path) {
                Ok(img) => {
                    let img = img.to_rgba8();
                    let (w, h) = (img.width(), img.height());
                    self.loading_img = Some((self.gui.add_image("loading", img.into_raw(), w, h, true), w, h));
                }
                Err(e) => eprintln!("{}: {e}", path.display()),
            }
        }
        if let Some(a) = &self.audio {
            a.play_startup_music(); // CharacterLoggedInMessage -> PlayStartupMusic (timing here: at the hand-off)
        }
    }

    fn loading_overlay(&mut self, list: &mut DrawList) {
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        let (black, a) = match self.fade {
            Fade::In(t) => (1.0, (t / FADE_IN).min(1.0)),
            Fade::Hold => (1.0, 1.0),
            Fade::Out(t) => {
                let a = (1.0 - t / FADE_OUT).max(0.0);
                (a, a)
            }
        };
        list.cmds.push(DrawCmd::Clip(None));
        list.cmds.push(DrawCmd::Solid { dst: [0.0, 0.0, w, h], color: [0, 0, 0], alpha: black });
        if let Some((id, iw, ih)) = self.loading_img {
            list.cmds.push(DrawCmd::Gfx { id, src: [0.0, 0.0, iw as f32, ih as f32], dst: [0.0, 0.0, w, h], tint: [255; 3], alpha: a });
        }
        let text = self.text.by_key(10000, "AO_Loading").unwrap_or_else(|| "..Anarchy Online is loading..".into());
        let tw = self.gui.text_width(FontId::TtMin12, &text);
        self.gui.text_cmds(FontId::TtMin12, &text, (self.size.0 as i32 - tw) / 2, self.size.1 as i32 - 50, 0xDDDDDD, a, list);
    }

    fn start_world_load(&mut self) {
        let Some(c) = self.selected.and_then(|i| self.chars.get(i)) else { return };
        let pf = &c.proxy.playfield;
        if pf.kind != PLAYFIELD_IDENTITY {
            eprintln!("playfield identity type {:#x} (expected {PLAYFIELD_IDENTITY:#x}); using instance {} as the id", pf.kind, pf.instance);
        }
        let (id, dir, tx) = (pf.instance as u32, self.dir.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let r = RecordStore::open(&dir)
                .and_then(|store| ao_formats::playfield::load_playfield_at(&store, &dir, id, ao_formats::playfield::DEFAULT_DAY_TIME))
                .map(Box::new)
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Bg::World(id, r));
        });
    }

    // ---- events ------------------------------------------------------------------------------------------------

    fn pump(&mut self, host: &mut Host) {
        while let Ok(bg) = self.rx.try_recv() {
            match bg {
                Bg::Servers(r) => {
                    match &r {
                        Ok(l) => eprintln!("servers: {}", l.iter().map(|s| format!("{} ({} online)", s.name, s.players)).collect::<Vec<_>>().join(", ")),
                        Err(e) => eprintln!("server list: {e}"),
                    }
                    self.servers = Some(r);
                }
                Bg::Backdrop(Ok(b)) => {
                    self.backdrop = Some(b);
                    let stage = u32::from(self.screen == Screen::CharSelect);
                    if matches!(self.screen, Screen::Login | Screen::Progress { .. } | Screen::CharSelect) {
                        self.show_backdrop(stage, host);
                    }
                }
                Bg::Backdrop(Err(e)) => eprintln!("login backdrop: {e}"),
                Bg::Connected(Ok(s)) => self.session = Some(s),
                Bg::Connected(Err(e)) => {
                    // ShowError(1,0): server not found
                    self.show_login(host);
                    self.message_box(&format!("Connection failed: {e}"));
                }
                Bg::World(id, Ok(scene)) => {
                    eprintln!("playfield {id} loaded");
                    self.world_scene = Some(scene);
                    self.world_ready = true;
                }
                Bg::World(id, Err(e)) => {
                    self.show_login(host);
                    self.message_box(&format!("Zone connected, but playfield {id} failed to load: {e}"));
                }
            }
        }
        while let Some(ev) = self.session.as_ref().and_then(|s| s.poll()) {
            match ev {
                LoginEvent::Status(s) => eprintln!("login: {s}"),
                LoginEvent::CharacterList(l) => {
                    let user = std::mem::take(&mut self.pending_user);
                    if !user.is_empty() {
                        self.prefs.remember(&user); // only after a successful login (LoginWindow_c::SlotLoginReply 0x0e)
                    }
                    self.show_characters(l, host);
                }
                LoginEvent::LoginError { code, message } => {
                    let text = if message.is_empty() { ao_net::client::login_error_text(code) } else { message };
                    self.show_login(host);
                    self.message_box(&text);
                }
                LoginEvent::ZoneHandoff { zone_ip, zone_port, .. } => {
                    eprintln!("zone hand-off to {zone_ip}:{zone_port}");
                    self.start_loading(host);
                }
                LoginEvent::ZoneConnected { messages } => {
                    self.zone_summary = messages.len();
                    eprintln!("zone connected, {} first frames", messages.len());
                    self.start_world_load();
                }
                LoginEvent::Disconnected(why) if self.screen != Screen::InWorld => {
                    self.show_login(host);
                    self.message_box(&format!("Disconnected: {why}"));
                }
                other => self.on_zone_event(other),
            }
        }
    }

    /// M3 seam: every session event once the world is shown. Zone message decoding/handling plugs in here.
    fn on_zone_event(&mut self, ev: LoginEvent) {
        match ev {
            LoginEvent::Disconnected(why) => self.in_world_msg = format!("Zone connection closed: {why}"),
            other => eprintln!("zone event (unhandled until M3): {other:?}"),
        }
    }

    fn handle(&mut self, ev: Event, host: &mut Host) {
        match ev {
            Event::CloseRequested { window } => {
                if Some(window) == self.login_w {
                    host.quit = true; // SlotQuitRequested -> shutdown
                } else if self.dialog_w.is_some_and(|d| d.0 == window) {
                    self.close_dialog();
                } // ProgressWindow swallows its close request
            }
            Event::Escape { window } => {
                if self.dialog_w.is_some_and(|d| d.0 == window) {
                    self.close_dialog();
                } else if Some(window) == self.char_w && self.dialog_w.is_none() {
                    self.show_login(host); // quit_btn / ESC -> Show(0)
                }
            }
            Event::TextChanged { window, view, text } if Some(window) == self.login_w => self.login_text_changed(window, &view, &text),
            Event::ComboChanged { window, view, text, .. } if Some(window) == self.login_w => self.login_text_changed(window, &view, &text),
            Event::Clicked { window, view, item } => {
                if Some(window) == self.login_w {
                    match view.as_str() {
                        "login_btn" => self.do_login(host),
                        "quit_btn" => host.quit = true,
                        "remove_btn" => {
                            let name = self.gui.text(window, "username");
                            self.prefs.remove(&name);
                            self.gui.combo_set_items(window, "username", self.prefs.accounts.clone());
                            self.gui.set_text(window, "username", "");
                        }
                        _ => {}
                    }
                } else if Some(window) == self.progress_w {
                    if view == "cancel_btn" {
                        self.show_login(host); // SlotLoginTimeout
                    }
                } else if self.dialog_w.is_some_and(|d| d.0 == window) {
                    let kind = self.dialog_w.map(|d| d.1);
                    match (view.as_str(), kind) {
                        ("ok_btn", Some(DialogKind::Activate)) => {
                            self.close_dialog();
                            self.enter_world(host);
                        }
                        ("cancel_btn", _) => self.close_dialog(),
                        _ => {}
                    }
                } else if Some(window) == self.char_w && self.dialog_w.is_none() {
                    match (view.as_str(), item) {
                        ("name_btn", Some(h)) => {
                            if let Some(i) = self.rows.iter().position(|r| r.handle == h) {
                                self.select_row(i, host);
                            }
                        }
                        ("login_btn", _) => self.play_pressed(host),
                        ("quit_btn", _) => self.show_login(host),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    fn close_dialog(&mut self) {
        if let Some((w, _)) = self.dialog_w.take() {
            self.gui.close_window(w);
        }
    }
}

impl Frontend for Play {
    fn gui(&self) -> &Gui {
        &self.gui
    }

    fn input(&mut self, ev: InputEvent, host: &mut Host) {
        match (self.screen, &ev) {
            (Screen::Loading, _) => return, // full-screen InvisibleButton swallows input
            (Screen::InWorld, InputEvent::Key { key: Key::Escape, pressed: true, .. }) => host.quit = true,
            (Screen::CharSelect, InputEvent::Key { key: Key::Up, pressed: true, .. }) if self.dialog_w.is_none() => return self.step_selection(-1, host),
            (Screen::CharSelect, InputEvent::Key { key: Key::Down, pressed: true, .. }) if self.dialog_w.is_none() => return self.step_selection(1, host),
            _ => {}
        }
        for e in self.gui.input(ev) {
            self.handle(e, host);
        }
    }

    fn frame(&mut self, dt: f32, size: (u32, u32), host: &mut Host) -> DrawList {
        self.time += dt;
        if size != self.size {
            let first = self.size == (0, 0);
            self.size = size;
            if let Some(w) = self.char_w {
                self.gui.resize_window(w, WindowSize::Fixed(size.0, size.1));
            }
            for w in [self.login_w, self.progress_w, self.dialog_w.map(|d| d.0)].into_iter().flatten() {
                self.recenter(w);
            }
            if first {
                match self.pending_fake.take() {
                    Some(i) => {
                        self.prefs.selected_character = i as i32;
                        self.show_characters(fake_list(), host);
                    }
                    None => self.show_login(host),
                }
            }
        }
        self.pump(host);
        if let (Screen::Progress { timeout, .. }, Some(w)) = (self.screen, self.progress_w) {
            self.progress_t += dt;
            self.gui.set_progress(w, "progress_bar", (self.progress_t / timeout).min(1.0));
            if self.progress_t >= timeout {
                self.show_login(host); // SlotLoginTimeout / SlotSelectCharacterTimeout
            }
        }
        if self.screen == Screen::CharSelect {
            self.tick_preview(dt, host);
        }
        match self.fade {
            Fade::In(t) if self.screen == Screen::Loading => {
                let t = t + dt;
                self.fade = if t >= FADE_IN { Fade::Hold } else { Fade::In(t) };
            }
            Fade::Hold if self.world_ready => {
                // world ready: StartClosingLoadscreen -> the loading screen dissolves into the world
                if let Some(s) = self.world_scene.take() {
                    let (eye, at) = match (s.spawn, s.spawn_look_at) {
                        (Some(e), Some(a)) => (Vec3::from(e), Vec3::from(a)),
                        (Some(e), None) => (Vec3::from(e), Vec3::from(e) + Vec3::Z),
                        _ => ao_render::default_view(&s),
                    };
                    host.set_scene(*s);
                    host.camera = Camera::look_at(eye, at);
                    self.screen = Screen::InWorld;
                    self.fade = Fade::Out(0.0);
                }
            }
            Fade::Out(t) => {
                let t = t + dt;
                if t >= FADE_OUT {
                    self.fade = Fade::Hold;
                    host.fly = true;
                } else {
                    self.fade = Fade::Out(t);
                }
            }
            _ => {}
        }
        if let Some(a) = &self.audio {
            a.update(dt, host.camera.pos.to_array(), ao_formats::playfield::DEFAULT_DAY_TIME);
        }
        let mut list = self.gui.frame(dt);
        let fading_out = matches!(self.fade, Fade::Out(_)) && self.screen == Screen::InWorld;
        if self.screen == Screen::Loading || fading_out {
            self.loading_overlay(&mut list);
        }
        list
    }
}
