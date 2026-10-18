//! The screens of docs/screens.md as behaviour on top of `ao_gui` windows.

use super::*;
use ao_formats::screens::{ao_to_render, set_login_stage, LOGIN_CAMERA};
use ao_net::msg::CharacterInfo;

// GC100b0125..100b01b3: native environment channels, not weather wind.
pub(super) fn environment_channels(phase: &mut f32, dt: f32) -> [f32; 3] {
    *phase = ((*phase as f64 + dt as f64 * 50.0) as f32 as f64 % 360.0) as f32;
    let channel = |offset: f64| {
        let angle = ((*phase as f64 + offset) * 3.140_000_104_904_175 / 180.0) as f32;
        (0.5 + (angle as f64).cos() as f32 as f64 * 0.5) as f32
    };
    [channel(90.0), 0.0, channel(180.0)]
}
#[cfg(test)]
#[test]
fn environment_effect_inputs_follow_native_oscillator_not_weather() {
    let mut phase = 0.0;
    let initial = environment_channels(&mut phase, 0.0);
    assert!((initial[0] - 0.500_398_16).abs() < 1e-6);
    assert!(initial[2] < 0.000_001);
    let advanced = environment_channels(&mut phase, 1.0);
    assert_eq!(phase, 50.0);
    assert!(advanced[0] < initial[0] && advanced[2] > initial[2]);
    environment_channels(&mut phase, 7.0);
    assert_eq!(phase, 40.0);
}


fn center(size: (u32, u32), outer: (u32, u32)) -> (i32, i32) {
    // `Window::MoveToCenter` [GUI 0x10154986]: floor(0.5 * screen - 0.5 * frame)
    (((size.0 as f32) * 0.5 - (outer.0 as f32) * 0.5).floor() as i32, ((size.1 as f32) * 0.5 - (outer.1 as f32) * 0.5).floor() as i32)
}

pub(super) fn breed_name(b: i32) -> &'static str {
    // Gamecode.dll GetBreedStr (docs/screens.md §5.3)
    match b {
        1 => "Solitus",
        2 => "Opifex",
        3 => "Nanomage",
        4 => "Atrox",
        _ => "Unknown",
    }
}

pub(super) fn sex_name(s: i32) -> &'static str {
    // Gamecode.dll GetSexStr table (~0x10032450): NONE, uni, male, female; the view upper-cases the first letter (docs/screens.md §5.3)
    match s {
        0 => "NONE",
        1 => "Uni",
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

fn limbo_timeout(timeout: i32) -> f32 {
    timeout.max(10) as f32
}

impl Play {
    // ---- backdrop ----------------------------------------------------------------------------------------------

    /// `LoginWorld_c::SetStage`: stage 0 for login/progress, 1 for character selection; camera fixed (docs/screens.md §2).
    fn show_backdrop(&mut self, stage: u32, host: &mut Host) {
        host.live_sky = Some(None);
        let Some(b) = &self.backdrop else { return };
        let mut s = (**b).clone();
        set_login_stage(&mut s, stage);
        self.mesh_base = s.meshes.len();
        host.camera = Camera::look_at(Vec3::from(s.spawn.unwrap_or_default()), Vec3::from(s.spawn_look_at.unwrap_or_default()));
        host.set_scene(s);
    }

    // ---- window helpers ----------------------------------------------------------------------------------------

    pub(super) fn open_centered(&mut self, view: &str) -> Option<WindowId> {
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

    pub(super) fn recenter(&mut self, w: WindowId) {
        let pos = center(self.size, self.gui.outer_size(w));
        self.gui.set_window_pos(w, pos);
    }

    /// `LoginWindow_c::Hide`: the window lives for the whole session.
    pub(super) fn hide_login(&mut self) {
        if let Some(w) = self.login_w {
            self.gui.set_window_visible(w, false);
        }
    }

    pub(super) fn close_all(&mut self) {
        self.hide_login();
        for w in [self.progress_w.take(), self.char_w.take(), self.dialog_w.take().map(|d| d.0)].into_iter().flatten() {
            self.gui.close_window(w);
        }
    }

    fn remember_login_user(&mut self) {
        let user = std::mem::take(&mut self.pending_user);
        if !user.is_empty() {
            self.prefs.remember(&user); // SlotLoginReply(0x0e / 0x4e), GUI 0x10011f8f
        }
    }

    fn show_limbo(&mut self, timeout: i32, info: CharacterInfo, host: &mut Host) {
        self.close_all();
        self.show_backdrop(0, host);
        self.screen = Screen::Limbo { timeout: limbo_timeout(timeout), character_id: info.id as u32 };
        self.progress_t = 0.0;
        let Some(w) = self.open_centered("LimboWindow") else { return };
        self.progress_w = Some(w);
        let profession = if info.profession < 1 || info.profession == 0xff {
            self.text.by_key(506, "NotChosenYet").unwrap_or_default()
        } else {
            self.text.by_id(2004, info.profession as u32).unwrap_or_default()
        };
        for (view, text) in [
            ("name", info.name), ("level", info.level.to_string()),
            ("gender", sex_name(info.gender).into()), ("breed", breed_name(info.breed).into()),
            ("profession", profession), ("location", info.area),
        ] {
            self.gui.set_text(w, view, &text);
        }
        self.gui.set_progress(w, "countdown_bar", 1.0);
        self.gui.relayout_window(w);
        self.recenter(w);
    }

    // ---- state 0: login ----------------------------------------------------------------------------------------

    /// `LoginModule_c::Show(0)` + `LoginWindow_c::Focus`.
    pub(super) fn show_login(&mut self, host: &mut Host) {
        self.save_viewer_cache();
        self.close_all();
        self.conn_gen += 1; // a connect still in flight is stale now (Bg::Connected is dropped)
        self.session = None; // dropping the session closes the connection (ResetConnectionAndConfig)
        self.loading_active = false;
        self.awaiting_alive = false;
        self.world_audio = None;
        if let Some(audio) = &self.audio {
            if std::env::var_os("AOMAC_AUDIO_LOG").is_some() {
                let before = audio.music_layer();
                audio.reset();
                eprintln!("audio login reset layer={before:?}->{:?} {}", audio.music_layer(), audio.status());
            } else {
                audio.reset();
            }
        }
        self.hud_pending.clear();
        if let Some(h) = self.hud.take() {
            h.close(&mut self.gui); // leaving the world
        }
        // `ActivateGameClosing(2)` (GUI 0x10028194): the chat windows close and the chat connection ends, the interaction windows go
        if let Some(mut c) = self.chat.take() {
            c.close(&mut self.gui, &self.text);
        }
        if let Some(mut i) = self.interact.take() {
            i.close_all(&mut self.gui);
        }
        if self.cc.is_some() {
            self.cc_close_windows();
            self.cc = None;
        }
        self.screen = Screen::Login;
        self.show_backdrop(0, host);
        // the original creates the LoginWindow once and Show/Hide it (docs/screens.md §1)
        let Some(w) = self.login_w.or_else(|| self.open_centered("LoginWindow")) else { return };
        self.login_w = Some(w);
        self.gui.set_window_visible(w, true);
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

    pub(super) fn request_servers(&mut self) {
        // Explicit retail-style login hosts do not depend on launcher discovery.
        let wanted = self.server_arg.as_deref().unwrap_or(&self.prefs.server);
        if self.servers_fetching || wanted.contains(':') {
            return;
        }
        self.servers_fetching = true;
        let (tx, dir) = (self.tx.clone(), prefs::dir());
        std::thread::spawn(move || {
            let result = ao_net::client::fetch_servers_with_config(dir.as_deref()).map_err(|e| format!("{e:#}"));
            let _ = tx.send(Bg::Servers(result));
        });
    }

    fn server(&mut self) -> Result<ServerEntry, String> {
        let wanted = self.server_arg.as_deref().unwrap_or(&self.prefs.server);
        if wanted.contains(':') {
            let endpoint = wanted.parse::<std::net::SocketAddrV4>()
                .map_err(|_| format!("Invalid login endpoint {wanted:?}; expected IPv4:port."))?;
            if endpoint.port() == 0 || endpoint.ip().is_unspecified() || endpoint.ip().is_multicast() || endpoint.ip().is_broadcast() {
                return Err(format!("Invalid login endpoint {wanted:?}."));
            }
            let server = ServerEntry { name: wanted.into(), ip: *endpoint.ip(), port: endpoint.port(), players: 0 };
            self.prefs.server = server.name.clone();
            return Ok(server);
        }
        let list = match self.servers.as_ref() {
            None => return Err("The server list has not arrived yet.".into()),
            Some(Err(e)) => return Err(format!("Server list unavailable: {e}")),
            Some(Ok(l)) => l,
        };
        let s = if wanted.is_empty() { list.first() } else { list.iter().find(|s| s.name.eq_ignore_ascii_case(wanted)) }
            .cloned().ok_or_else(|| if wanted.is_empty() { "No servers online.".into() } else { format!("Selected server {wanted:?} is not available.") })?;
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
        // One retry per new login request, never from the per-frame pump.
        if matches!(self.servers, Some(Err(_))) {
            self.request_servers();
        }
        let server = match self.server() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{e}");
                return self.show_error(1, 0);
            }
        };
        eprintln!("connecting to {} ({}:{}, {} online)", server.name, server.ip, server.port, server.players);
        self.pending_user = user.clone();
        self.login_cred = Some((user.clone(), pass.clone()));
        self.gui.set_text(w, "password", ""); // ResetConnectionAndConfig clears name/password
        self.show_progress(CONNECT_TIMEOUT, false, host);
        let (tx, gen) = (self.tx.clone(), self.conn_gen);
        // connect blocks (<= 10 s); the password only lives in this closure until the session thread owns it
        std::thread::spawn(move || {
            // AOMAC_NET_TRACE=<file>: record every frame (credentials/cookies redacted) for protocol work
            let r = match std::env::var_os("AOMAC_NET_TRACE") {
                Some(p) => LoginSession::connect_traced(&server, ao_net::conn::record_tap(p.into())),
                None => LoginSession::connect(&server),
            }
            .inspect(|s| s.login(&user, &pass))
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Bg::Connected(gen, r));
        });
    }

    // ---- state 1 / 4: progress ---------------------------------------------------------------------------------

    fn show_progress(&mut self, timeout: f32, joining: bool, host: &mut Host) {
        self.hide_login();
        for w in [self.progress_w.take(), self.char_w.take()].into_iter().flatten() {
            self.gui.close_window(w);
        }
        self.screen = Screen::Progress { timeout, joining };
        self.progress_t = 0.0;
        if !joining {
            self.show_backdrop(0, host);
        }
        self.progress_w = self.open_centered("ProgressDialog");
    }

    /// `LoginModule_c::ShowError(code, arg)` [GUI 0x10011deb] (docs/screens.md §3.6): the original navigates its embedded
    /// browser to `<ERRORURL><code>[-<arg>].html`; there is no embedded browser here, so the same URL opens in the default one.
    fn show_error(&mut self, code: u32, arg: i32) {
        let Some(base) = errorurl(&self.dir) else {
            return eprintln!("login error {code}-{arg}: AnarchyLauncher.url has no ERRORURL");
        };
        let url = if arg != 0 { format!("{base}{code}-{arg}.html") } else { format!("{base}{code}.html") };
        eprintln!("login error page: {url}");
        self.opened_urls.push(url.clone());
        if cfg!(test) {
            return; // tests never launch a browser
        }
        if let Err(e) = std::process::Command::new("open").arg(&url).status() {
            eprintln!("cannot open {url}: {e}");
        }
    }

    /// Notice box for non-login errors (the client's own text strings / playfield load failures).
    pub(super) fn message_box(&mut self, text: &str) {
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

    fn apply_login_audio_prefs(&self) {
        if let Some(audio) = &self.audio {
            let root = super::prefs::dir();
            let account = self.prefs.accounts.get(self.prefs.selected_account).map(String::as_str);
            audio.set_prefs(&super::options::login_audio_prefs(&self.dir, root.as_deref(), account));
        }
    }

    pub(super) fn show_characters(&mut self, list: CharacterList, host: &mut Host) {
        // Account preferences are available before character creation starts playing sounds.
        self.apply_login_audio_prefs();
        self.char_list = list.clone();
        self.hide_login();
        for w in [self.progress_w.take(), self.char_w.take()].into_iter().flatten() {
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
            g.set_item_selected(h, false);
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
        // New Character: enabled iff activeCount < slotCount (`CharSelectWindow_c::SlotCharListReceived`)
        g.set_enabled(win, "create_btn", active < self.slots);
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
            self.gui.set_item_selected(r.handle, k == i);
        }
        self.gui.relayout_window(win);
        self.gui.set_enabled(win, "login_btn", true);
        self.gui.set_enabled(win, "delete_btn", true);
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
        self.worker = Some(preview::Worker::start(self.dir.clone(), breed, sex, self.chars[i].id, self.chars[i].info.head));
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
        // Preserve queued First/Clip messages until the backdrop can accept their mesh topology.
        let Some(b) = &self.backdrop else { return };
        while let Ok(m) = wk.rx.try_recv() {
            match m {
                preview::Out::First(mut ch) => {
                    let mut s = (**b).clone();
                    set_login_stage(&mut s, 1);
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
    fn show_loading(&mut self, host: &mut Host) {
        self.hide_login();
        for w in [self.progress_w.take(), self.char_w.take(), self.dialog_w.take().map(|d| d.0)].into_iter().flatten() {
            self.gui.close_window(w);
        }
        self.screen = Screen::Loading;
        self.fade = Fade::In(0.0);
        host.fly = false;
        // `ServerLogin3DModule_t::SetLoadingScreen(n)`: after a creation `welcome_to_rubika.jpg` (docs/screens.md §7)
        let image = if std::mem::take(&mut self.welcome_image) { "welcome_to_rubika.jpg" } else { "ai_loading_login.png" };
        if self.loading_name != image {
            self.loading_img = None;
            self.loading_name = image;
        }
        if self.loading_img.is_none() {
            let path = self.dir.join("cd_image/gui/Default/gfx").join(image);
            match image::open(&path) {
                Ok(img) => {
                    let img = img.to_rgba8();
                    let (w, h) = (img.width(), img.height());
                    self.loading_img = Some((self.gui.add_image("loading", img.into_raw(), w, h, true), w, h));
                }
                Err(e) => eprintln!("{}: {e}", path.display()),
            }
        }
    }

    /// The hand-off (`LoginEvent::ZoneHandoff`): loading screen plus `CharacterLoggedInMessage` -> `PlayStartupMusic`.
    pub(super) fn start_loading(&mut self, host: &mut Host) {
        self.show_loading(host);
        self.loading_active = true;
        self.apply_login_audio_prefs();
        if let Some(a) = &self.audio {
            a.play_startup_music(); // CharacterLoggedInMessage -> PlayStartupMusic (timing here: at the hand-off)
        }
    }

    /// The server moves us to another playfield while we are in the world (`n3TeleportIIR_t` with a destination, a second
    /// `PlayfieldAnarchyFIIR_t`, or a zone redirection): `FlowControlModule_t::TeleportStartedMessage` [GUI 0x1002910e], posted
    /// every frame while the old playfield is stopped (`PlayfieldAnarchy_t::Run` [GC 0x101225b3] -> event 5 -> AFCM 0x145) and
    /// guarded by `m_isTeleporting`. There is **no loading screen** (program 5 is only added at login / character creation) and
    /// the HUD and chat windows stay: it prints `ChangingArea` in red, removes the target (AFCM 0x1e/0x112), hides the 3D
    /// world (`DisplaySystem+0x44 = 0`, the GUI stays; black behind it is [INFERENCE]) and locks the input. The old dynels die
    /// (`n3Playfield_t::StopPlayfield`); `CharInPlay` is owed again after `TeleportEnded` (docs/zone/world.md §10.2).
    fn begin_zone_change(&mut self, host: &mut Host) {
        self.teleport_started(host);
        // `StopPlayfield` runs once per playfield: a second event before the new world exists finds nothing left to stop
        if !std::mem::take(&mut self.world_ready) {
            return;
        }
        self.save_viewer_cache();
        self.drop_player();
        host.fly = false;
        self.zone.reset_world();
        self.fight_reset();
        self.interact_reset();
        self.world_frames = IN_PLAY_FRAMES;
        self.world_seconds = IN_PLAY_SECONDS;
        self.world_scene = None;
        self.world_ground = None;
        self.ground_texture_patches.clear();
        self.ground_texture_serial = self.ground_texture_serial.wrapping_add(1);
        host.ground_textures = None;
        self.fade = Fade::Hold;
    }

    /// Drops the current `Player`, parking its warmed `ActorAssets` in `self.zone.world` first
    /// ([`player::Player::into_avatar_assets`]) so the next zone's/re-placement's `Player::new` reuses them
    /// instead of resolving the own model's rest pose from scratch again.
    fn drop_player(&mut self) {
        if let Some(player) = self.player.take() {
            self.zone.world.stash_avatar_assets(player.into_avatar_assets());
        }
        self.player_attempt_serial = None;
    }

    /// `FlowControlModule_t::TeleportStartedMessage` (GUI event 5), also posted by `TeleportTrier_t::StartTryingTeleport` of the client-initiated
    /// path while the playfield keeps running: guarded by `m_isTeleporting`; prints `ChangingArea`, drops the target, stops the user input
    /// (`InputConfig+0x18`) and switches the 3D viewport off until `AliveMessage` ([`Play::alive`]).
    fn teleport_started(&mut self, host: &mut Host) {
        if std::mem::replace(&mut self.teleporting, true) {
            return;
        }
        if let (Some(c), Some(t)) = (self.chat.as_mut(), self.text.by_key(110, "ChangingArea")) {
            c.system_line(&mut self.gui, &t, 12);
        }
        self.zone.set_target(None);
        self.awaiting_alive = true;
        host.look = false;
    }

    /// `TeleportEndedMessage` (GUI event 6) when no new world follows (`TeleportTrier_t::TeleportFailed`): guarded by `m_isTeleporting`; the
    /// `Entering ...` line and the `CharInPlay` countdown start again (the viewport and the input wait for `AliveMessage` as ever).
    fn teleport_ended(&mut self) {
        if !std::mem::take(&mut self.teleporting) {
            return;
        }
        self.entering_text();
        self.world_frames = IN_PLAY_FRAMES;
        self.world_seconds = IN_PLAY_SECONDS;
        self.zone.in_play_sent = false;
    }

    /// `FlowControlModule_t::AliveMessage` [GUI 0x10028543], posted by the server's echo of our `CharInPlayIIR_t` for the own character
    /// (`CharInPlayIIR_t::Activate` [GC 0x1007264d]): `SetStaticInputMode(8)`, `Activate3DViewPort` (the world is drawn again),
    /// `EnableUserInput`, and the `CharInPlay` countdown flag (`DAT_102760cd`) clears. The original has no timeout: without the echo the
    /// viewport stays off and the input stopped after a teleport.
    fn alive(&mut self) {
        if !self.world_ready || self.world_scene.is_some() || self.teleporting {
            return;
        }
        // AliveMessage sends AFCM 0x1b:0x135 only while the loading module is active.
        if self.loading_active {
            self.fade = Fade::Out(0.0);
        }
        self.awaiting_alive = false;
        self.zone.in_play_sent = true;
    }

    fn in_play_due(&mut self, dt: f32) -> bool {
        if self.screen != Screen::InWorld || !self.world_ready || self.zone.in_play_sent || self.teleporting {
            return false;
        }
        self.world_frames = self.world_frames.saturating_sub(1);
        self.world_seconds -= dt;
        (self.world_frames < 0 || self.world_seconds < 0.0) && !self.zone.world.has_workload() && self.zone.own().is_some()
    }

    /// Whether the game keys / mouse reach the [`Player`](player::Player): `InputConfig+0x18` (`isUserInputStopped`, = `awaiting_alive`) is
    /// clear. `InputConfig_t::FrameProcess` [GUI 0x1001ae14] drops the whole input queue while it is set (except events `0x8002a` /
    /// `0xe002f`), the GUI's included; the port keeps the GUI input alive (a missing echo must not lock the window) and holds back only the
    /// game input. Releases always pass: `EnableUserInput` resets the key states, which the port replaces by never dropping a release.
    fn game_input_open(&self) -> bool {
        !self.awaiting_alive
    }

    /// `AOMAC_ZONE_TIMING`: starts a fresh zone-change cycle timer (unless the current one began under 3 s ago — the coalescing window for
    /// a `ZoneRedirect` followed by its `Playfield`) and logs `label` at t=0 (or the elapsed time since the redirect). Which stage fires
    /// last (`CharInPlay`'s server echo vs. the dynels already drawn) is not fixed, so the cycle is time-boxed rather than closed by an event.
    fn zt_begin(&mut self, label: &str) {
        if !self.zone_timing { return; }
        if self.zone_timer.is_none_or(|t0| t0.elapsed() > std::time::Duration::from_secs(3)) {
            self.zone_timer = Some(std::time::Instant::now());
            self.zone_timing_dynels_seen = false;
        }
        self.zt(label);
    }

    /// `AOMAC_ZONE_TIMING`: logs `label` with milliseconds elapsed since the current zone-change cycle began.
    fn zt(&self, label: &str) {
        if let (true, Some(t0)) = (self.zone_timing, self.zone_timer) {
            eprintln!("zone_timing {label} t={}ms", t0.elapsed().as_millis());
        }
    }

    /// `TeleportEndedMessage`: `EnteringPF` ("Entering '%s'") with the playfield name (`N3Msg_GetPFName`), or `EnteringNewArea` when the
    /// playfield has no name or `N3Msg_IsDungeon` (`Report::dungeon`), as a red System line. The name comes from `pfnrmap.dat` like the
    /// character list's (`FUN_1003676c`, the lookup `GetPFName` tries first, then the playfield's own name).
    fn entering_text(&mut self) {
        let line = self.entering_line();
        if let (Some(c), Some(t)) = (self.chat.as_mut(), line) {
            c.system_line(&mut self.gui, &t, 12);
        }
    }

    /// The text of [`Play::entering_text`].
    fn entering_line(&self) -> Option<String> {
        match self.zone.playfield.and_then(|p| self.pf_names.get(&p)).filter(|_| !self.dungeon) {
            Some(n) => self.text.by_key(110, "EnteringPF").map(|t| t.replace("%s", n)),
            None => self.text.by_key(110, "EnteringNewArea"),
        }
    }

    pub(super) fn loading_overlay(&mut self, list: &mut DrawList) {
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

    /// `PlayfieldAnarchyFIIR_t` arrived: load that playfield in the background (the loading screen stays up until it is ready).
    fn start_world_load(&mut self, id: u32) {
        self.world_sky = None;
        self.world_weather = None;
        self.world_lightning = super::lightning::LightningFx::default();
        self.world_life = None;
        self.life_dangers.clear();
        let (dir, tx, want_audio) = (self.dir.clone(), self.tx.clone(), self.audio.is_some());
        let (texture_quality, ground_texture_quality) = (self.texture_quality, self.ground_texture_quality);
        // the zone clock and game day (`GameTimeIIR_t`) when the burst reached the playfield message; later `GameTime`s resync the live sky
        let (day_time, day) = (self.zone.day_time(), self.zone.game_day as u32);
        let area_playfield = self.zone.area_playfield;
        let (zt_on, zt0) = (self.zone_timing, self.zone_timer);
        let zt = move |label: &str| if let (true, Some(t0)) = (zt_on, zt0) { eprintln!("zone_timing {label} t={}ms", t0.elapsed().as_millis()); };
        std::thread::spawn(move || {
            let mut r = RecordStore::open(&dir).and_then(|store| {
                store.set_other_texture_quality(texture_quality);
                store.set_ground_texture_quality(ground_texture_quality);
                let is_grid = area_playfield.map(|id| ao_formats::playfield::area::load(&store, id))
                    .transpose()?.flatten().is_some_and(|areas| areas.is_grid());
                let (mut scene, report) = ao_formats::playfield::load_playfield_report_on_day(&store, &dir, id, day_time, day)?;
                zt("pf_terrain_statics_loaded");
                if !scene.ground_detail.is_empty() {
                    if let Ok(collision) = ao_formats::playfield::collision::Collision::load(&store, id) {
                        super::player::bake_sky_open(&collision, &mut scene);
                    }
                    zt("pf_sky_open_baked");
                }
                let weather = ao_formats::playfield::open_weather(&store, id)?;
                let lightning = super::lightning::LightningFx::load(&store)?;
                let _ = tx.send(Bg::Weather(id, Box::new((weather, lightning))));
                let life = (|| -> anyhow::Result<_> {
                    let mut world = native_life_spawn::LifeWorld::load(&store, id)?;
                    let mut models = native_life_render::LifeModels::default();
                    for index in 0..world.catalog.len() {
                        if !world.catalog_used(index) { continue; }
                        let mesh = world.catalog[index].mesh;
                        models.load(&store, mesh)?;
                        world.set_mesh_height(mesh, models.height(mesh).unwrap());
                        world.set_mesh_animation(mesh, models.boundaries(mesh), models.total_time(mesh).unwrap());
                    }
                    Ok((world, models))
                })().map(Box::new).map_err(|error| format!("{error:#}"));
                zt("pf_life_loaded");
                let _ = tx.send(Bg::Life(id, life));
                // `PlayfieldInit` [GC 0x10016e2c] -> `SandyInterfaceModule_t::ActivateGameZone`: district music, ambience and statel emitters
                let audio = want_audio.then(|| ao_audio::PlayfieldAudio::load(&store, id, &report.sounds).map_err(|e| eprintln!("playfield audio {id}: {e:#}")).ok()).flatten();
                Ok((scene, report, audio, is_grid))
            });
            // the Map window's ground image comes from the scene just built (`Report::ground`), not from a second load (docs/gui.md 12)
            let ground = r.as_ref().ok().and_then(|(scene, report, ..)| hud::ground_map(scene, report));
            let _ = tx.send(Bg::Ground(id, ground.map(Box::new)));
            if let Ok((_, report, audio, _)) = &mut r {
                let _ = tx.send(Bg::Info(id, report.dungeon, audio.take().map(Box::new)));
            }
            let r = r.map(|(scene, _, _, is_grid)| (Box::new(scene), is_grid)).map_err(|e| format!("{e:#}"));
            match ao_formats::playfield::SkyClock::open(&dir, id) {
                Ok(mut clock) => {
                    if let Some(clock) = &mut clock { clock.set_texture_quality(texture_quality); }
                    drop(tx.send(Bg::Sky(id, Box::new(clock))));
                }
                Err(e) => eprintln!("live sky of playfield {id}: {e:#}"),
            }
            zt("pf_thread_done");
            let _ = tx.send(Bg::World(id, area_playfield, ground_texture_quality, r));
        });
    }

    fn refresh_ground_quality(&mut self) {
        if self.screen != Screen::InWorld || self.ground_texture_patches.is_empty() || self.ground_texture_installed_quality == self.ground_texture_quality { return }
        let Some(id) = self.zone.playfield else { return };
        self.ground_texture_installed_quality = self.ground_texture_quality;
        self.ground_texture_serial = self.ground_texture_serial.wrapping_add(1);
        let (serial, quality) = (self.ground_texture_serial, self.ground_texture_quality);
        let (dir, tx, patches) = (self.dir.clone(), self.tx.clone(), self.ground_texture_patches.clone());
        std::thread::spawn(move || {
            let result = super::open_texture_store(&dir).and_then(|store| {
                store.set_ground_texture_quality(quality);
                ao_formats::playfield::refresh_ground_textures(&store, id, &patches)
            }).map_err(|e| format!("{e:#}"));
            let _ = tx.send(Bg::GroundTextures(serial, result));
        });
    }

    // ---- events ------------------------------------------------------------------------------------------------

    fn pump(&mut self, host: &mut Host) {
        while let Ok(bg) = self.rx.try_recv() {
            match bg {
                Bg::Servers(r) => {
                    self.servers_fetching = false;
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
                Bg::CcWorld(r) => self.cc_world_loaded(r, host),
                Bg::Connected(gen, _) if gen != self.conn_gen => {} // cancelled/timed out: dropping the result closes the connection
                Bg::Connected(_, Ok(s)) => self.session = Some(s),
                Bg::Connected(_, Err(e)) => {
                    eprintln!("connection failed: {e}");
                    self.show_login(host);
                    self.show_error(1, 0); // ConnectToLH failed -> ShowError(1,0)
                }
                Bg::Sky(id, c) if Some(id) == self.zone.playfield => self.world_sky = *c,
                Bg::Sky(..) => {}
                Bg::Weather(id, w) if Some(id) == self.zone.playfield => {
                    let (weather, lightning) = *w;
                    self.world_weather = Some(weather);
                    self.world_lightning = lightning;
                }
                Bg::Weather(..) => {}
                Bg::Life(id, result) if Some(id) == self.zone.playfield => {
                    match result {
                        Ok(boxed) => {
                            let (mut life, models) = *boxed;
                            life.set_enabled(self.wildlife_enabled);
                            self.world_life = Some((life, models));
                        }
                        Err(error) => eprintln!("wildlife {id}: {error}"),
                    }
                }
                Bg::Life(..) => {}
                Bg::Ground(id, g) if Some(id) == self.zone.playfield => self.world_ground = g.map(|g| (id, g)),
                Bg::Ground(..) => {}
                Bg::Info(id, dungeon, audio) if Some(id) == self.zone.playfield => {
                    self.dungeon = dungeon;
                    self.world_audio = audio.map(|a| (id, a));
                }
                Bg::Info(..) => {}
                // a load that the server has since replaced by another playfield is dropped
                Bg::World(id, area, _, _) if Some(id) != self.zone.playfield || area != self.zone.area_playfield => {}
                Bg::World(id, _, quality, Ok((scene, is_grid))) => {
                    eprintln!("playfield {id} loaded");
                    self.zt("world_data_ready");
                    self.ground_texture_installed_quality = quality;
                    self.zone.is_grid = is_grid;
                    self.world_scene = Some(scene);
                    self.world_ready = true;
                }
                Bg::World(id, _, _, Err(e)) => {
                    self.show_login(host);
                    self.message_box(&format!("Zone connected, but playfield {id} failed to load: {e}"));
                }
                Bg::GroundTextures(serial, result) if serial == self.ground_texture_serial && self.screen == Screen::InWorld => match result {
                    Ok(textures) => host.ground_textures = Some(textures),
                    Err(e) => eprintln!("ground texture quality refresh: {e}"),
                },
                Bg::GroundTextures(..) => {}
            }
        }
        while let Some((ev, replayed)) = self.fake_events.pop_front().map(|ev| (ev, true))
            .or_else(|| self.session.as_ref().and_then(|s| s.poll()).map(|ev| (ev, false))) {
            if self.screen == Screen::Create && self.create_session_event(&ev, host) {
                continue;
            }
            if let LoginEvent::CharacterDeleted { character_id } = &ev {
                self.character_deleted(*character_id as i32, host);
                continue;
            }
            match ev {
                LoginEvent::Status(s) => eprintln!("login: {s}"),
                LoginEvent::CharacterList(l) => {
                    self.remember_login_user();
                    self.show_characters(l, host);
                }
                LoginEvent::ProtocolNotOk => {
                    self.show_login(host);
                    self.message_box("Connection error:\nProtocol Not OK.");
                    if let Some((_, kind)) = self.dialog_w.as_mut() {
                        *kind = DialogKind::Fatal;
                    } else {
                        host.quit = true;
                    }
                }
                LoginEvent::LoginLimbo { timeout, info } => {
                    self.show_limbo(timeout, info, host);
                    self.remember_login_user();
                }
                LoginEvent::LoginError { code, message } => {
                    // SlotLoginReply: type 0x0d (LoginError) -> ShowError(type, arg)
                    eprintln!("login error {code}: {message}");
                    self.show_login(host);
                    self.show_error(0x0d, code as i32);
                }
                LoginEvent::Rejected { code, detail } => {
                    // SlotLoginReply: type 0x21 (RequestRejected) -> ShowError(type, detail)
                    eprintln!("login rejected (system message {code:#x}, detail {detail})");
                    self.show_login(host);
                    self.show_error(code, detail);
                }
                LoginEvent::ZoneHandoff { zone_ip, zone_port, character_id } => {
                    eprintln!("zone hand-off to {zone_ip}:{zone_port}");
                    self.zone = zone::Zone::new(character_id);
                    // a fresh session: nothing of an earlier world; the playfield may finish loading while the creation cinematic still runs
                    (self.world_ready, self.world_scene, self.world_ground) = (false, None, None);
                    self.awaiting_alive = true;
                    self.teleporting = false;
                    self.fight_reset();
                    self.interact_reset();
                    let mut chat = chat::Chat::new(!self.fake && !replayed);
                    if let Some((u, p)) = &self.login_cred {
                        chat.set_credentials(u, p);
                    }
                    self.chat = Some(chat);
                    self.zone.world.start(self.dir.clone(), character_id as i32);
                    self.world_frames = IN_PLAY_FRAMES;
                    self.world_seconds = IN_PLAY_SECONDS;
                    // creating a character: the exit cinematic runs first, `exit_done` opens the loading screen (docs/screens.md §12)
                    if self.screen != Screen::Create {
                        self.start_loading(host);
                    }
                }
                LoginEvent::ZoneFrame(f) => {
                    let combat_event_start = self.fight.as_ref().map_or(0, |m| m.pending_events().len());
                    if let Some(m) = self.fight.as_mut() {
                        if let Some(p) = self.player.as_ref() {
                            m.set_third_person(p.camera_mode() != 0);
                        }
                        m.on_frame(&f);
                    }
                    // CharacterAction 0x7a posts CancelCampMessage (GC 0x1005dae8 -> event 0x2a -> AFCM 10:0x20).
                    if let Ok(message) = ao_net::n3::decode(&f) {
                        if message.header.target.kind == 50000 && message.header.target.instance == self.zone.char_id as i32
                            && matches!(message.body, ao_net::n3::N3::World(ao_net::n3::world::World::CharacterAction(a)) if a.action == 0x7a)
                        {
                            self.cancel_camp();
                        }
                    }
                    self.interact_zone_frame(&f);
                    if hud::Hud::wants_zone_frame(&f) {
                        match self.hud.as_mut() {
                            Some(h) => h.on_zone_frame(&mut self.gui, &f, &mut self.zone, |id| self.chat.as_ref().is_some_and(|chat| chat.is_ignored(id))),
                            None => self.hud_pending.push(f.clone()),
                        }
                    }
                    // Stat feedback needs the old values; InfoPacket pages need the applied values.
                    if let Some(c) = self.chat.as_mut() {
                        if let Some(hud) = self.hud.as_ref() { c.configure_escape(&hud.dvalues); }
                        c.on_zone_frame(&mut self.gui, &f, &self.zone, &self.text, self.hud.as_mut().map(|hud| &mut hud.winb.perks));
                    }
                    let own_events_before = self.zone.own_events.len();
                    let zone_event = self.zone.on_frame(&f);
                    // Native attack applies Health immediately; the next packet's chat delta must see it.
                    if let Some(m) = self.fight.as_ref() {
                        for event in &m.pending_events()[combat_event_start..] {
                            combat::glue::sync_stats(&mut self.zone, event);
                        }
                    }
                    // GUI 0x10027d47: only the own, successfully decoded resurrection message.
                    if self.zone.own_events.get(own_events_before..).is_some_and(|events| events.iter().any(|event| matches!(event, zone::OwnEvent::Resurrected))) {
                        if let Some(hud) = self.hud.as_mut() {
                            hud.trigger_tip(&mut self.gui, "OnResurrect", "");
                        }
                    }
                    // Spell instances apply against this message's stats, before any
                    // later packet changes their level/source skill in the same batch.
                    if let Some(hud) = self.hud.as_mut() {
                        if let Some(health) = hud.nano_stats_changed(&mut self.zone) {
                            if let Some(combat) = self.fight.as_mut() { combat.set_health_local(health); }
                        }
                    } else {
                        // Login headers initialize health before the in-world projection exists.
                        self.zone.nano_stat_removals.clear();
                    }
                    if let Some(c) = self.chat.as_mut() {
                        c.on_zone_applied(&mut self.gui, &f, &self.zone, &self.text);
                    }
                    match zone_event {
                        zone::ZoneEvent::Playfield(id) => {
                            eprintln!("zone: playfield {id}");
                            if self.screen == Screen::InWorld {
                                self.zt_begin(&format!("recv_playfield id={id}"));
                                self.begin_zone_change(host);
                            }
                            self.start_world_load(id);
                        }
                        // `n3TeleportIIR_t` (own, destination playfield): `StartTeleport` -> `TeleportStarted`, before the new playfield arrives
                        zone::ZoneEvent::Teleport => {
                            if self.screen == Screen::InWorld {
                                self.zt_begin("recv_teleport");
                                self.begin_zone_change(host);
                            }
                        }
                        // the server's echo of our `CharInPlay`: `AliveMessage`
                        zone::ZoneEvent::Alive => { self.zt("server_alive"); self.alive(); }
                        // `GameTime_t::Update`: the clock jumps to the server's; the loading world already used the older one
                        zone::ZoneEvent::Time => host.sky_clock = Some(self.zone.day_time()),
                        zone::ZoneEvent::Reloading => {
                            if let Some(c) = self.chat.as_mut() { c.feedback(&mut self.gui, "Feedback_Reloading", &self.text); }
                        }
                        zone::ZoneEvent::None => {}
                    }
                }
                LoginEvent::ZoneRedirect { zone_ip, zone_port } => {
                    // the session thread has reconnected (system message 0x3C); the new server's burst follows
                    eprintln!("zone redirection to {zone_ip}:{zone_port}");
                    if self.screen == Screen::InWorld {
                        self.zt_begin("recv_redirect");
                        self.begin_zone_change(host);
                    }
                }
                LoginEvent::Disconnected(why) => {
                    // ServerLostMessage [GUI 0x10028d50] -> ActivateGameClosing [0x10028194]:
                    // an active timed logout selects its destination; otherwise this is status 3, server lost.
                    eprintln!("connection lost: {why}");
                    self.session = None;
                    self.camp = None;
                    self.camp_bar_close();
                    match self.logout.expired() {
                        Some(super::logout::Closing::System) => host.quit = true,
                        Some(super::logout::Closing::Login) => self.show_login(host),
                        None if !host.quit => {
                            self.logout.cancel();
                            self.show_login(host);
                            self.show_error(3, 0);
                        }
                        None => {}
                    }
                }
                other => eprintln!("unhandled session event: {other:?}"),
            }
        }
    }

    fn handle(&mut self, ev: Event, host: &mut Host) {
        match ev {
            Event::Copy(text) => {
                if let Err(e) = arboard::Clipboard::new().and_then(|mut c| c.set_text(text)) {
                    eprintln!("clipboard copy: {e}");
                }
            }
            Event::PasteRequested => match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
                Ok(t) => {
                    for e in self.gui.input(InputEvent::Paste(t)) {
                        self.handle(e, host);
                    }
                }
                Err(e) => eprintln!("clipboard paste: {e}"),
            },
            Event::CloseRequested { window } => {
                if Some(window) == self.login_w {
                    host.quit = true; // SlotQuitRequested -> shutdown
                } else if self.dialog_w.is_some_and(|d| d.0 == window) {
                    if self.dialog_w.is_some_and(|d| d.1 == DialogKind::Fatal) {
                        host.quit = true;
                    }
                    self.close_dialog();
                } else if Some(window) == self.progress_w && matches!(self.screen, Screen::Limbo { .. }) {
                    self.show_login(host);
                } // ProgressWindow swallows its close request
            }
            Event::Escape { window } => {
                if self.dialog_w.is_some_and(|d| d.0 == window && d.1 == DialogKind::ExitCc) {
                    self.cc_exit_answer(false, host);
                } else if self.dialog_w.is_some_and(|d| d.0 == window) {
                    if self.dialog_w.is_some_and(|d| d.1 == DialogKind::Fatal) {
                        host.quit = true;
                    }
                    self.close_dialog();
                    self.create_message_closed();
                } else if Some(window) == self.char_w && self.dialog_w.is_none() {
                    self.show_login(host); // quit_btn / ESC -> Show(0)
                }
            }
            Event::TextChanged { window, view, text } if Some(window) == self.login_w => self.login_text_changed(window, &view, &text),
            Event::TextChanged { window, view, text } if view == "name" && self.screen == Screen::Create => {
                self.create_text_changed(window, &text);
            }
            Event::TextChanged { window, view, text } if view == "name_input" && self.dialog_w.is_some_and(|d| d.0 == window) => {
                self.gui.set_enabled(window, "ok_btn", !text.is_empty()); // CharDeleteWindow_c::SlotTextEdited
            }
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
                    if let Screen::Limbo { character_id, .. } = self.screen {
                        if view == "login_btn" {
                            if let Some(session) = &self.session {
                                session.select_character(character_id);
                            }
                            self.show_progress(JOIN_TIMEOUT, true, host);
                        }
                    }
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
                        ("ok_btn", Some(DialogKind::ExitCc)) => self.cc_exit_answer(true, host),
                        ("cancel_btn", Some(DialogKind::ExitCc)) => self.cc_exit_answer(false, host),
                        ("ok_btn", Some(DialogKind::Delete)) => self.delete_ok(host),
                        ("cancel_btn", Some(DialogKind::Fatal)) => {
                            self.close_dialog();
                            host.quit = true;
                        }
                        ("cancel_btn", Some(DialogKind::Message)) => {
                            self.close_dialog();
                            self.create_message_closed();
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
                        ("create_btn", _) => self.start_creation(host),
                        ("delete_btn", _) => self.delete_pressed(),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn close_dialog(&mut self) {
        if let Some((w, kind)) = self.dialog_w.take() {
            self.gui.close_window(w);
            if let (DialogKind::Message, Some(under)) = (kind, self.under_dialog.take()) {
                self.dialog_w = Some((under, DialogKind::Delete));
                self.gui.focus(under, "name_input");
            }
        }
    }
}

impl Frontend for Play {
    fn mover_sound(&mut self, event: ao_scene::mover::SoundEvent, host: &Host) {
        if let Some(audio) = &self.audio {
            if let Err(error) = audio.play_mover_sound(event, host.camera.pos.to_array()) {
                eprintln!("traffic sound: {error:#}");
            }
        }
    }

    fn gui(&self) -> &Gui {
        &self.gui
    }

    /// Audit item 2 (Enhanced "Interface Scale"): read once at `Play::new` from the account-wide root `Prefs.xml`, so
    /// the viewer can size the login screen's window correctly on its very first frame, before any HUD/DValues refresh.
    fn interface_scale(&self) -> u8 {
        self.interface_scale
    }

    fn input(&mut self, ev: InputEvent, host: &mut Host) {
        #[cfg(test)]
        let mut input_timing = tests::InputTiming::start();
        if let Some(hud) = self.hud.as_ref() {
            if let Some(chat) = self.chat.as_mut() { chat.configure_escape(&hud.dvalues); }
            if let Some(interact) = self.interact.as_mut() { interact.configure_escape(&hud.dvalues); }
        }
        if self.screen == Screen::InWorld && matches!(ev, InputEvent::Key { .. } | InputEvent::Text(_) | InputEvent::MouseDown { .. } | InputEvent::MouseUp { .. } | InputEvent::MouseMove { .. } | InputEvent::Wheel { .. }) {
            if let Some(c) = self.chat.as_mut() {
                let sneak = self.player.as_ref().is_some_and(|p| p.mode() == 6);
                c.afk_activity(&mut self.gui, &self.zone, &self.text, sneak);
            }
        }
        let open = self.game_input_open();
        #[cfg(test)]
        tests::InputTiming::mark(&mut input_timing, 0);
        if let (Screen::InWorld, Some(p)) = (self.screen, self.player.as_mut()) {
            if let InputEvent::MouseDown { x, y, .. } | InputEvent::MouseUp { x, y, .. } | InputEvent::Wheel { x, y, .. } = ev {
                if open || matches!(ev, InputEvent::MouseUp { .. }) {
                    let wants_mouse = self.gui.wants_mouse(x, y);
                    #[cfg(test)]
                    tests::InputTiming::mark(&mut input_timing, 1);
                    p.mouse(&ev, wants_mouse, host);
                }
            }
        }
        #[cfg(test)]
        tests::InputTiming::mark(&mut input_timing, 2);
        self.interact_mouse(&ev, host);
        #[cfg(test)]
        tests::InputTiming::mark(&mut input_timing, 3);
        match (self.screen, &ev) {
            (Screen::Loading, _) => return, // full-screen InvisibleButton swallows input
            (Screen::Create, _) if self.create_input(&ev, host) => return,
            // `CharCreateModule_t::SlotEscPressed` (camera-tool command 0x31) and `DialogBox_c::SlotEscPressed` hear the same global signal
            (Screen::Create, InputEvent::Key { key: Key::Escape, pressed: true, .. }) => {
                if let Some(c) = self.cc.as_mut() {
                    c.esc();
                }
            }
            // TextInputMode suppresses the global hotkey. InfoViewMode uses CloseInfoSystem only;
            // dialog listeners otherwise share the broadcast with target removal and HUD windows.
            (Screen::InWorld, InputEvent::Key { key: Key::Escape, pressed: true, .. })
                if !self.gui.text_focused() && self.chat.as_ref().is_some_and(|c| c.esc_closes() || c.info_view_open()) => {
                if let Some(c) = self.chat.as_mut() {
                    let info_mode = c.info_view_open();
                    c.escape(&mut self.gui, &self.zone, &self.text);
                    if info_mode {
                        return;
                    }
                }
            }
            (Screen::CharSelect, InputEvent::Key { key: Key::Up, pressed: true, .. }) if self.dialog_w.is_none() => return self.step_selection(-1, host),
            (Screen::CharSelect, InputEvent::Key { key: Key::Down, pressed: true, .. }) if self.dialog_w.is_none() => return self.step_selection(1, host),
            _ => {}
        }
        let default_keys;
        let fixed = match self.hud.as_ref() {
            Some(h) => h.key_tables().1,
            None => {
                default_keys = super::options::keys::FixedKeys::default();
                &default_keys
            }
        };
        if self.screen == Screen::InWorld && self.chat.as_mut().is_some_and(|c| c.input(&mut self.gui, &ev, &self.zone, &self.text, fixed)) {
            return;
        }
        #[cfg(test)]
        tests::InputTiming::mark(&mut input_timing, 4);
        if let Some(h) = self.hud.as_mut() {
            h.input(&mut self.gui, &mut self.zone, self.player.as_ref(), &ev, host.mods);
            host.pointer_warp = h.take_pointer_warp();
            // CTRL / ALT + left click on a character: select (done) and `N3Msg_SwitchTarget` (`FUN_1002c469`)
            let clicked = h.take_click();
            if let (Some(id), Some(p)) = (clicked, self.player.as_ref()) {
                if p.attack_modifier() && self.zone.dynels.contains_key(&id) {
                    if let Some(m) = self.fight.as_mut() {
                        m.command(combat::module::Command::SwitchTarget(id), &self.zone, p.mode());
                    }
                }
            }
            if let Some(id) = clicked {
                self.interact_left_click(id);
            }
            // TAB cycles the target (`COMMAND_NEXT_HOSTILE_TARGET`, only outside text input): it must not also move the GUI focus into the chat input
            if matches!(ev, InputEvent::Key { key: Key::Tab, .. }) && self.screen == Screen::InWorld && !self.gui.text_focused() {
                return;
            }
        }
        #[cfg(test)]
        tests::InputTiming::mark(&mut input_timing, 5);
        for e in self.gui.input(ev) {
            if self.chat.as_mut().is_some_and(|c| c.event(&mut self.gui, &e, &self.zone, &self.text)) {
                continue;
            }
            if self.hud.as_mut().is_some_and(|h| h.event(&mut self.gui, &e, &self.zone)) {
                continue;
            }
            if self.interact_event(&e) {
                continue;
            }
            self.handle(e, host);
        }
        #[cfg(test)]
        tests::InputTiming::mark(&mut input_timing, 6);
    }

    fn game_input(&mut self, ev: ao_render::GameInput, host: &mut Host) {
        let open = self.game_input_open();
        if open && self.screen == Screen::InWorld {
            if let Some(chat) = self.chat.as_mut() {
                chat.afk_activity(&mut self.gui, &self.zone, &self.text, self.player.as_ref().is_some_and(|p| p.mode() == 6));
            }
        }
        if matches!(ev, ao_render::GameInput::FocusLost) {
            if let Some(hud) = self.hud.as_mut() { hud.cancel_key_input(); }
        }
        if let (Screen::InWorld, ao_render::GameInput::Key { code, pressed: false, .. }, Some(h)) = (self.screen, ev, self.hud.as_mut()) {
            if open {
                if let Some(key) = super::controls::key_id(code) {
                    h.key_release(&self.gui, key, host.mods);
                }
            }
        }
        // the window / hotbar hot keys and the options window's key capture read the physical key (any key can be bound, `options/keys.rs`)
        if let (Screen::InWorld, ao_render::GameInput::Key { code, pressed: true, repeat }, Some(h)) = (self.screen, ev, self.hud.as_mut()) {
            if let Some(key) = super::controls::key_id(code) {
                if open && !repeat && h.key_press(&mut self.gui, key, host.mods) {
                    return; // captured for a binding: the key does not act
                }
                // `KEY_COMMAND_CHAT_HISTORY_PAGE_UP / DOWN` (`RepeatMode`, also while typing): a page through the chat history
                let input = super::options::keys::key_input(key, host.mods);
                let (up, down) = (h.key_tables().1.get("KEY_COMMAND_CHAT_HISTORY_PAGE_UP"), h.key_tables().1.get("KEY_COMMAND_CHAT_HISTORY_PAGE_DOWN"));
                if input == up || input == down {
                    if let Some(c) = self.chat.as_mut() {
                        c.scroll_history(&mut self.gui, input == down);
                    }
                }
            }
        }
        if let (Screen::InWorld, Some(p)) = (self.screen, self.player.as_mut()) {
            if open || matches!(ev, ao_render::GameInput::Key { pressed: false, .. } | ao_render::GameInput::FocusLost) {
                let first_person = p.camera_mode() == 0;
                p.game_input(ev);
                // GUI 0x10027c9f: ToggleCameraView, not next/previous camera vehicle slots.
                if first_person != (p.camera_mode() == 0) {
                    if let Some(hud) = self.hud.as_mut() {
                        hud.trigger_tip(&mut self.gui, "OnChangeCameraMode", "");
                    }
                }
            }
        }
    }

    fn frame(&mut self, dt: f32, size: (u32, u32), host: &mut Host) -> DrawList {
        host.post_focus = None;
        if self.player.is_none() {
            host.grass_exposure = None;
            host.grass_benders.clear();
        }
        if let Some(hud) = self.hud.as_ref() {
            self.texture_quality = hud.dvalues.get_i64("DisplayOtherTextureQuality").unwrap_or(3) as i32;
            self.texture_compressed = hud.dvalues.flag("DisplayCompressedTextures");
            self.ground_texture_quality = hud.dvalues.get_i64("GroundTextureQuality").unwrap_or(2) as i32;
            self.wildlife_enabled = hud.dvalues.flag("Wildlife");
            // aomac Enhanced options page (not retail): `options.rs::enhanced_page` radio indices -> concrete renderer values.
            const ANISOTROPY: [u16; 5] = [1, 2, 4, 8, 16];
            const MSAA: [u32; 4] = [1, 2, 4, 8];
            let aniso = hud.dvalues.get_i64("AomacTextureSharpening").unwrap_or(0).clamp(0, 4) as usize;
            let msaa = hud.dvalues.get_i64("AomacEdgeSmoothing").unwrap_or(0).clamp(0, 3) as usize;
            host.anisotropy = Some(ANISOTROPY[aniso]);
            host.msaa_samples = Some(MSAA[msaa]);
            // Live switches warm unseen pipeline variants on a worker; fixtures retain the immediate channel.
            host.enhanced_switch = Some(hud.dvalues.enhanced_settings());
            let lut_root = hud.dvalues.main_prefs_dir();
            if host.grade_lut_root.as_deref() != lut_root {
                host.grade_lut_root = lut_root.map(std::path::Path::to_path_buf);
            }
            // live-apply while in game; the initial value (login screen) came from `Play::new`'s startup read.
            self.interface_scale = hud.dvalues.get_i64("AomacInterfaceScale").unwrap_or(0).clamp(0, 3) as u8;
            host.borderless_game = hud.dvalues.flag("AomacBorderlessFullscreen");
            host.pinch_zoom = hud.dvalues.flag("AomacPinchZoom");
            self.smooth_text = hud.dvalues.flag("AomacSmoothText");
        }
        // audit item 2: every screen, including the login screen before any HUD exists.
        host.interface_scale = Some(self.interface_scale);
        // Apply saved text preferences to login/character screens too; AA is independent of GUI scale.
        self.gui.set_text_scale(host.window_scale.clamp(1, 8) as u8, self.smooth_text);
        self.gui.set_screen_size(size.0, size.1);
        if let Some((life, models)) = &mut self.world_life {
            life.set_enabled(self.wildlife_enabled);
            if !self.wildlife_enabled { models.clear(); }
        }
        self.refresh_ground_quality();
        // Ground invalidates current materials; ordinary callbacks affect resource uploads.
        // Colour depth remains startup-only.
        let better_mips = self.hud.as_ref().is_some_and(|h| h.dvalues.flag("AomacBetterMipFiltering"));
        host.texture_policy = Some((self.texture_resolution, self.texture_compressed, better_mips));
        self.zone.world.set_texture_quality(self.texture_quality);
        if let Some(player) = self.player.as_mut() {
            player.set_texture_quality(self.texture_quality);
            if player.take_screenshot() {
                // Retail F12 (`GUI.dll` `FlowControlModule_t::SlotScreenshot`): silent, no chat feedback (RE confirmed
                // no feedback string exists); `next_screenshot_path` matches the retail folder/naming.
                if let Some(path) = super::prefs::next_screenshot_path() { host.screenshot = Some(path); }
            }
        }
        if let Some(hud) = self.hud.as_ref() {
            if let Some(chat) = self.chat.as_mut() { chat.configure_escape(&hud.dvalues); }
            if let Some(interact) = self.interact.as_mut() { interact.configure_escape(&hud.dvalues); }
        }
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
        if let Some(state)=self.zone.world.shared_native_fog(){host.bind_native_fog(state);}
        if let Some(h)=&self.hud {host.set_native_fog_preference(h.dvalues.prefs.get_int("FogMode",super::dvalue::Kind::Login).unwrap_or(3));}
        self.pump(host);
        // A zone load may replace the effects renderer during pump.
        if let Some(state)=self.zone.world.shared_native_fog(){host.bind_native_fog(state);}
        if let (Screen::Progress { timeout, .. }, Some(w)) = (self.screen, self.progress_w) {
            self.progress_t += dt;
            self.gui.set_progress(w, "progress_bar", (self.progress_t / timeout).min(1.0));
            if self.progress_t >= timeout {
                self.show_login(host); // SlotLoginTimeout / SlotSelectCharacterTimeout
            }
        }
        if let (Screen::Limbo { timeout, .. }, Some(w)) = (self.screen, self.progress_w) {
            self.progress_t += dt;
            self.gui.set_progress(w, "countdown_bar", 1.0 - (self.progress_t / timeout).min(1.0));
            if self.progress_t >= timeout {
                self.show_login(host); // LimboWindow_c::SlotFrameProcess 0x1001016a
            }
        }
        if self.screen == Screen::CharSelect {
            self.tick_preview(dt, host);
        }
        self.zone.tick(dt);
        if let Some(hud) = self.hud.as_mut() {
            if let Some(health) = hud.tick_stat_effects(&mut self.zone, f64::from(dt)) {
                if let Some(combat) = self.fight.as_mut() { combat.set_health_local(health); }
            }
        }
        match self.fade {
            Fade::In(t) if self.screen == Screen::Loading => {
                let t = t + dt;
                self.fade = if t >= FADE_IN { Fade::Hold } else { Fade::In(t) };
            }
            Fade::Hold if self.world_ready => {
                // Install the world behind the loading module; only AliveMessage closes it.
                if let Some(s) = self.world_scene.take() {
                    // the player's own dynel (SimpleCharFullUpdate, arrives with the zone burst) decides; the scene's
                    // density-based spawn is only the fallback when the server sent none
                    let (eye, at) = match (self.zone.own(), s.spawn, s.spawn_look_at) {
                        (Some(d), ..) => {
                            let e = Vec3::from(zone::scene_pos(d.pos)) + Vec3::Y * EYE_HEIGHT;
                            (e, e + Vec3::from(zone::scene_forward(d.yaw.unwrap_or(0.0))))
                        }
                        (None, Some(e), Some(a)) => (Vec3::from(e), Vec3::from(a)),
                        (None, Some(e), None) => (Vec3::from(e), Vec3::from(e) + Vec3::Z),
                        _ => ao_render::default_view(&s),
                    };
                    self.zone.world.lens = s.lens.unwrap_or_default();
                    self.ground_texture_serial = self.ground_texture_serial.wrapping_add(1);
                    self.ground_texture_patches = s.ground_detail.iter().map(|patch| (patch.mesh, patch.origin, patch.cells)).collect();
                    host.ground_textures = None;
                    host.set_scene(*s);
                    // the live sky follows the server's `GameTime` from here on (1 `GameDayTime` second per real second)
                    let sky = self.world_sky.take().map(|c| c.on_day(self.zone.game_day as u32));
                    host.live_sky = Some(sky.map(|mut c| {
                        let pose = Box::new(c.pose_source());
                        ao_render::LiveSky { start: self.zone.day_time(), scale: 1.0, source: Box::new(move |t| c.at(t)), pose: Some(pose) }
                    }));
                    host.camera = Camera::look_at(eye, at);
                    // `PlayfieldInit` -> `SandyInterfaceModule_t::ActivateGameZone`: the district music / ambience / emitters of the new playfield
                    // replace the old ones (none: leave the playfield, nothing keeps playing from the previous zone)
                    let pf_audio = self.world_audio.take().map(|(_, p)| *p);
                    if let Some(a) = &self.audio {
                        a.set_playfield(pf_audio);
                    }
                    self.screen = Screen::InWorld;
                    self.zt("first_frame");
                    // `TeleportEndedMessage` [GUI 0x100292ce]: the flag clears and the countdown to `CharInPlay` starts again; the HUD and
                    // chat windows of the old world are still there (docs/zone/world.md §10.2)
                    let teleported = std::mem::take(&mut self.teleporting);
                    if self.hud.is_none() {
                        match hud::Hud::new(&mut self.gui, &self.dir, self.size) {
                            Ok(h) => self.hud = Some(h),
                            Err(e) => eprintln!("hud: {e:#}"),
                        }
                        // `LoadUserConfig` (GUI 0x1006bacd): the account's / character's prefs files over the template defaults
                        if let (Some(h), Some(d), Some(a)) = (self.hud.as_mut(), super::prefs::dir(), self.prefs.accounts.get(self.prefs.selected_account)) {
                            h.dvalues.open_user(&d, a, self.zone.char_id);
                            h.prefs_loaded(&mut self.gui);
                        }
                        if let Some(h) = self.hud.as_mut() {
                            for f in std::mem::take(&mut self.hud_pending) {
                                h.on_zone_frame(&mut self.gui, &f, &mut self.zone, |id| self.chat.as_ref().is_some_and(|chat| chat.is_ignored(id)));
                            }
                        }
                    }
                    // GUI 0x10027d26 passes GetPFName, including named dungeon playfields.
                    if let (Some(hud), Some(name)) = (self.hud.as_mut(), self.zone.playfield.and_then(|id| self.pf_names.get(&id))) {
                        hud.trigger_tip(&mut self.gui, "OnEnteringPlayfield", name);
                    }
                    if let (Some(h), Some((id, g))) = (self.hud.as_mut(), self.world_ground.take()) {
                        h.provide_ground(id, g.0, g.1);
                    }
                    // after the HUD: the chat windows draw above its bar windows (as in the original, whose bar windows are backmost)
                    if let (Some(c), false) = (self.chat.as_mut(), teleported) {
                        if let Some(hud) = self.hud.as_ref() { c.configure_escape(&hud.dvalues); }
                        let char_dir = self.hud.as_ref().and_then(|h| h.dvalues.char_dir());
                        if let Err(e) = c.open(&mut self.gui, self.size, char_dir) {
                            eprintln!("chat: {e:#}");
                        }
                    }
                    if teleported {
                        self.entering_text();
                    }
                    if !teleported {
                        self.gui.clear_focus(); // the login window's password field kept the keyboard focus
                    }
                    self.drop_player();
                    host.fly = teleported && self.player.is_none();
                    self.world_frames = IN_PLAY_FRAMES;
                    self.world_seconds = IN_PLAY_SECONDS;
                }
            }
            Fade::Out(t) => {
                let t = t + dt;
                if t >= FADE_OUT {
                    self.fade = Fade::Hold;
                    self.loading_active = false;
                    host.fly = self.player.is_none(); // free-fly only when the avatar could not be built
                } else {
                    self.fade = Fade::Out(t);
                }
            }
            _ => {}
        }
        if self.screen == Screen::InWorld && self.world_ready && self.world_scene.is_none() && self.zone.own().is_some()
            && self.player.as_ref().map(|p| p.serial()).or(self.player_attempt_serial) != Some(self.zone.own_serial)
        {
            // SetMainDynel (GC 0x10019f77): the own SCFU can arrive after world installation.
            // Remember failed asset builds too, rather than attempting heavy construction every frame.
            self.drop_player();
            self.player_attempt_serial = Some(self.zone.own_serial);
            let playfield = self.zone.playfield.unwrap_or(0);
            self.player = player::Player::new(&self.dir, &mut self.zone, playfield);
            if self.player.is_some() {
                host.fly = false;
            }
            if !self.zone.in_play_sent {
                self.world_frames = IN_PLAY_FRAMES;
                self.world_seconds = IN_PLAY_SECONDS;
            }
        }
        if self.in_play_due(dt) {
                // GUI 0x10027d73: clear static mode 0x3a after sending, but input still waits for Alive.
                self.zone.in_play_sent = true;
                if let Some(s) = &self.session {
                    let id = self.zone.char_id;
                    s.send_zone(ao_net::n3::outgoing::n3_frame(0, id, ao_net::n3::outgoing::char_in_play(id as i32)));
                    eprintln!("zone: CharInPlay sent ({} frames, {} dynels known)", self.zone.frames, self.zone.dynels.len());
                    self.zt("char_in_play_sent");
                }
        }
        // `screen == InWorld` is already true throughout a mid-game teleport (no loading screen, docs/zone/world.md §10.2); gate on
        // `world_ready` (set once the destination's geometry/`Bg::World` arrived) so this does not fire on the pre-teleport dynel list.
        // One-shot per cycle (the condition stays true for many frames); the cycle itself is time-boxed by `zt_begin`, not closed here.
        if self.world_ready && self.zone_timer.is_some() && !self.zone_timing_dynels_seen && !self.zone.dynels.is_empty() {
            self.zone_timing_dynels_seen = true;
            self.zt(&format!("dynels_visible n={}", self.zone.dynels.len()));
        }
        if self.screen == Screen::InWorld {
            // Complete camp before world updates can emit more sounds into the login lifecycle.
            self.camp_frame(dt, host);
        }
        if self.screen == Screen::InWorld {
            // `ViewDistance` / `DisplayCharViewDistance` (docs/chat/dvalue.md): far plane + fog + statel LOD, characters' draw distance
            if let Some(h) = self.hud.as_mut() {
                h.sync_keys();
                // aomac Enhanced "Widescreen FOV" (audit item 16, not retail, default off, PRK-signed-off): widens the
                // real lens and, so the perf pre-skin cull never gets stricter than it, `Dynels::fov_horizontal` too.
                let widescreen_fov = h.dvalues.flag("AomacWidescreenFOV");
                if let Some(p) = self.player.as_mut() {
                    let (b, f) = h.key_tables();
                    p.set_keys(b, f);
                    p.set_view_distance(h.dvalues.view_distance());
                    p.set_widescreen_fov(widescreen_fov);
                    p.set_scenery_distance_mult(h.dvalues.get_f32("AomacSceneryDistance").unwrap_or(1.0));
                    if let Some(mode) = h.take_menu_camera() {
                        p.select_camera_mode(mode);
                    }
                    h.sync_camera_mode(p.camera_mode());
                    p.set_control_prefs(&super::controls::ControlPrefs::from_dvalues(&h.dvalues));
                    p.set_fade(super::avatar::Fade::from_prefs(&h.dvalues.prefs));
                }
                self.zone.world.char_view_distance = h.dvalues.char_view_distance();
                self.zone.world.fov_horizontal = super::camera::horizontal_fov(widescreen_fov, size.0 as f32 / size.1.max(1) as f32);
                self.zone.world.set_name_prefs(&h.dvalues);
                self.zone.triangle_targeting = h.dvalues.get_i64("BoundingBoxTargeting").unwrap_or(1) == 0;
                host.spaceships_shown = h.dvalues.prefs.get_int("IsSpaceShipsShown", super::dvalue::Kind::Char).unwrap_or(1) != 0;
                host.smooth_animations = h.dvalues.prefs.get_int("SmoothAnimations", super::dvalue::Kind::Login).unwrap_or(1) != 0;
                host.realistic_water = h.dvalues.prefs.get_int("RealisticWater", super::dvalue::Kind::Login).unwrap_or(1) != 0;
                host.cloud_preferences = Some((
                    h.dvalues.prefs.get_int("RealisticClouds", super::dvalue::Kind::Login).unwrap_or(1) != 0,
                    h.dvalues.prefs.get_int("SimpleClouds", super::dvalue::Kind::Login).unwrap_or(1) != 0,
                ));
                self.zone.world.set_shadows(h.dvalues.prefs.get_int("Shadows", super::dvalue::Kind::Login).unwrap_or(1) != 0);
                // Actual preference changes invoke native SetFogMode, not per-frame resets.
                host.set_native_fog_preference(h.dvalues.prefs.get_int("FogMode", super::dvalue::Kind::Login).unwrap_or(3));
                host.wait_for_vert_sync = Some(h.dvalues.prefs.get_int("WaitForVertSync", super::dvalue::Kind::Char).unwrap_or(0) != 0);
                host.mouse_lag_fix = h.dvalues.prefs.get_int("MouseLagFix", super::dvalue::Kind::Char).unwrap_or(0) != 0;
                host.ground_detail = Some((
                    h.dvalues.get_i64("DisplayGroundQuality").unwrap_or(200).clamp(100, 200) as i32,
                    h.dvalues.get_i64("DisplayGroundFullQualityRadius").unwrap_or(44).clamp(7, 44) as i32,
                ));
                host.ground_rendering = h.dvalues.prefs.get_int("GroundRendering", super::dvalue::Kind::Login).unwrap_or(3).clamp(0, 3);
            }
            if let Some(p) = self.player.as_mut() {
                for f in p.frame(dt, host, &mut self.zone, self.gui.text_focused()) {
                    if let Some(s) = &self.session {
                        s.send_zone(f);
                    }
                }
            }
            // `TeleportTrier_t` of the client-initiated path: `StartTryingTeleport` posts event 5, `TeleportFailed` event 6 and the feedback line
            for ev in std::mem::take(&mut self.zone.teleport_events) {
                match ev {
                    zone::TeleportEvent::Started => self.teleport_started(host),
                    zone::TeleportEvent::Failed => {
                        self.teleport_ended();
                        if let Some(c) = self.chat.as_mut() {
                            c.feedback(&mut self.gui, "Feedback_AreaChangeNotInitiated", &self.text);
                        }
                    }
                }
            }
            if self.awaiting_alive {
                host.look = false; // `InputConfig+0x18`: the mouse look is stopped with the rest of the user input
            }
            if let Some(weather) = &mut self.world_weather {
                weather.update(self.zone.game_day as u32, self.zone.day_time() as f64, dt);
                if let Some(audio) = &self.audio {
                    audio.update_weather(&weather.state());
                }
                host.effect_wind = weather.state().wind;
                let enabled = weather.state().lightning_enabled();
                let extent = self.player.as_ref().and_then(|p| p.effect_surface()).and_then(|c| c.borrow().tile_size()).map(ao_formats::lightning::extent).unwrap_or(0.0);
                let controlled = host.camera.pos.to_array();
                let strikes = self.world_lightning.tick(enabled, dt, controlled, extent, controlled[1]);
                host.actor_models.extend(self.world_lightning.take_model_upload());
                host.actors.extend(self.world_lightning.actors());
                for strike in &strikes {
                    let cue = strike.audio(controlled, extent);
                    weather.lightning_impulse(cue.distance);
                    if let Some(audio) = &self.audio {
                        if let Err(error) = audio.play_lightning_cue(cue.volume, cue.delay) {
                            eprintln!("lightning cue: {error:#}");
                        }
                    }
                }
                let offset = weather.state().quake_offset;
                super::camera::apply_ground_shake(&mut host.camera, glam::Vec3::new(offset[0], offset[1], -offset[2]));
            }
            host.effect_day_time = Some(self.zone.day_time());
            host.effect_game_seconds = self.zone.game_seconds();
            host.effect_first_person = self.player.as_ref().is_some_and(|player| player.camera_mode() == 0);
            host.effect_environment_wind = Some(environment_channels(&mut self.effect_environment_phase, dt));
            host.effect_environment_center = self.player.as_ref().map(|_| host.camera.pos.to_array());
            host.effect_playfield = self.player.as_ref().and_then(|player| player.effect_surface()).and_then(|surface| surface.borrow().effect_resource());
            self.zone.world.set_collision(self.player.as_ref().and_then(|player| player.effect_surface()));
            self.zone.world.refresh_effect_source_runtime();
            let breed = self.zone.stat(4);
            let in_play=self.zone.stat(super::zone::IN_PLAY_STAT).is_some_and(|value|value!=0);
            if let Some(player) = self.player.as_ref() { player.prepare_effect_source(&mut self.zone.world, host, breed, in_play); }
            self.use_action_frame(dt);
            self.fight_frame(dt);
            {
                let own=self.zone.char_id as i32;
                let player=self.player.as_ref();
                let stats=&self.zone.stats;
                let character_stats=&self.zone.character_stats;
                // GC1003a04f emits filename+Identity on GlobalSignals+190. No retail
                // subscriber was found; do not invent UI/file/numeric sound playback.
                drop(self.zone.world.server_effect_frame(
                    |anchor| player?.effect_anchor(anchor).map(|matrix| glam::Mat4::from_cols_array_2d(&matrix)),
                    |who,stat| {
                        if who==own {stats.get(&stat).copied()} else {None}
                            .or_else(|| character_stats.get(&who)?.get(&stat).copied())
                    },
                ));
            }
            let mut collision = self.player.as_ref().and_then(|p| p.effect_surface())
                .map(|surface| move |p| super::player::effect_collision(&surface.borrow(), p));
            let query = collision.as_mut().map(|query| query as &mut dyn FnMut(ao_render::Vec3) -> Option<(ao_render::Vec3, ao_render::Vec3)>);
            let player = self.player.as_ref();
            self.zone.world.update_with_collision(dt, host.camera.pos.to_array(), host.camera.forward().to_array(), host, query, |id| {
                player?.effect_anchor(id).map(|matrix| glam::Mat4::from_cols_array_2d(&matrix))
            });
            if host.enhanced().is_some_and(|s| s.foliage_enabled() || s.interaction_enabled()) {
                host.grass_wind = self.world_weather.as_ref().map_or([0.0; 3], |weather| weather.state().wind);
                host.grass_benders.retain(|bender| bender.actor.is_none());
                host.grass_benders.extend(self.zone.world.grass_characters()
                    .map(|(id, position)| ao_render::GrassBender { actor: Some(id), position }));
            }
            if let (Some(control), Some(surface), Some((life, models))) = (
                self.zone.own().map(|own| glam::Vec3::from_array(own.pos)),
                self.player.as_ref().and_then(|player| player.effect_surface()),
                self.world_life.as_mut(),
            ) {
                if let Some(a) = &self.audio {
                    self.life_dangers.extend(a.take_life_dangers().into_iter().map(|(pos, radius)| {
                        super::native_life::CharacterSensor { pos: glam::Vec3::new(pos[0], pos[1], -pos[2]), radius }
                    }));
                }
                self.zone.world.life_sensors(control, &mut self.life_characters, &mut self.life_dead);
                life.tick(dt, control, &surface.borrow(), &self.life_characters, &self.life_dangers, &self.life_dead, glam::Vec3::from_array(host.effect_wind));
                self.life_dangers.clear(); // GC100b85a3, after the complete Life::Run pass
                models.retain(&life.ids);
                for (&id, species) in life.ids.iter().zip(&life.species) {
                    let mesh = life.catalog[species.catalog_index].mesh;
                    models.update(id, mesh, species.pos, species.rotation, species.animation_time, species.visible);
                }
                models.upload(host);
                host.actors.extend(models.frames().cloned());
            }
            match self.zone.world.effect_camera_offset(host.camera.pos) {
                Ok(offset) => super::camera::apply_ground_shake(&mut host.camera, offset),
                Err(error) => eprintln!("effect camera: {error:#}"),
            }
            for sound in self.zone.world.take_effect_sounds() {
                if let Some(audio) = &self.audio {
                    // SI100071ed rounds the authored radius with x87 round(radius - 0.49999).
                    let radius = (sound.parameters[1] as f64 - 0.49999).round_ties_even() as u16;
                    let parameters = [sound.parameters[0], radius as f32, sound.parameters[2], sound.parameters[3]];
                    match audio.play_effect_sound(sound.id, sound.pos, sound.velocity, host.camera.pos.to_array(), parameters, sound.probability) {
                        Ok(voices) => {
                            if std::env::var_os("AOMAC_AUDIO_LOG").is_some() {
                                eprintln!("effect sound {:#x} at {:?}: {} voice(s), radius {}", sound.id, sound.pos, voices.len(), radius);
                            }
                        }
                        Err(error) => eprintln!("effect sound: {error:#}"),
                    }
                } else {
                    eprintln!("effect sound {:#x}: audio runtime unavailable", sound.id);
                }
            }
            // `Door_t` open / close: `PlayGameSound(id, door position)` (docs/zone/doors.md §5); the fight sounds (combat/notes.rs) go the same way
            for s in self.zone.world.take_sounds() {
                if let Some(a) = &self.audio {
                    let voices = a.play_game_sound_with(s.id, s.pos, host.camera.pos.to_array(), s.material, s.size);
                    if std::env::var_os("AOMAC_AUDIO_LOG").is_some() {
                        eprintln!("game sound {} at {:?}: {} voice(s) (material {}, size {})", s.id, s.pos, voices.len(), s.material, s.size);
                    }
                }
            }
            for (id, pos, duration, volume, selector, who) in self.zone.world.take_nano_sounds() {
                if let Some(audio) = &self.audio {
                    let voices = audio.play_game_sound_duration(id, pos, host.camera.pos.to_array(), duration, volume);
                    if log_nano_sound(duration, &voices) && std::env::var_os("AOMAC_AUDIO_LOG").is_some() {
                        eprintln!("nano sound who={who} stage={selector:#x} id={id:#x} at {pos:?} duration={duration} volume={volume} voices={voices:?}");
                    }
                }
            }
        }
        if let Some(a) = &self.audio {
            // `SoundOptionsMonitor_c` (GUI 0x100c4cc9): master / FX / music volume, the on-off switches and `BattlemusicMode`, live from the options window
            if let Some(h) = self.hud.as_ref() {
                a.set_prefs(&super::options::audio_prefs(&h.dvalues));
            }
            a.update(dt, host.camera.pos.to_array(), self.zone.day_time());
        }
        // ActionMenu RunChatScript uses the same command path as a line entered in chat.
        for script in self.hud.as_mut().map(|h| h.take_menu_scripts()).unwrap_or_default() {
            if let Some(c) = self.chat.as_mut() {
                c.run_line(&mut self.gui, &script, &self.zone, &self.text);
            }
        }
        if let Some(c) = self.chat.as_mut() {
            c.resize(&mut self.gui, size);
            c.update(&mut self.gui, dt, &self.text);
            if let Some(hud) = self.hud.as_mut() {
                for (event, data) in c.take_tip_events() {
                    hud.trigger_tip(&mut self.gui, event, &data);
                }
            }
            if let Some(s) = &self.session {
                for f in c.take_outbox() {
                    s.send_zone(f);
                }
            }
        }
        // `/quit` and `/open` `/close` `/toggle` of the chat input (docs/chat/dialogs.md §3)
        let (wins, quit) = self.chat.as_mut().map(|c| (c.take_windows(), c.take_quit())).unwrap_or_default();
        if quit {
            self.quit_cmd(host);
        }
        // the options window's `Quit2Windows` / `Quit2Login` buttons = AFCM 0x133 / 0x134, like `/quit` / `/camp`
        for a in self.hud.as_mut().map(|h| h.take_option_actions()).unwrap_or_default() {
            match a {
                super::options::Action::Quit => self.quit_cmd(host),
                super::options::Action::Camp => self.camp(),
            }
        }
        if let Some(h) = self.hud.as_mut() {
            use super::chat::WindowOp;
            for (dv, op) in wins {
                if let Some(k) = hud::WindowKind::from_dvalue(dv) {
                    match op {
                        WindowOp::Open => h.open(&mut self.gui, k),
                        WindowOp::Close => h.close_kind(&mut self.gui, k),
                        WindowOp::Toggle => h.toggle(&mut self.gui, k),
                    }
                } else if h.dvalues.exists(dv) {
                    let value = match op {
                        WindowOp::Open => true,
                        WindowOp::Close => false,
                        WindowOp::Toggle => !h.dvalues.flag(dv),
                    };
                    h.dvalues.set(dv, super::dvalue::Variant::Bool(value));
                }
            }
        }
        // `/option` `/setoption` `/dvalue` `/chardist` `/viewdist` `/char&viewdist` (docs/chat/dvalue.md): the store lives in the HUD
        let mut config_saved = false;
        if let (Some(c), Some(h)) = (self.chat.as_mut(), self.hud.as_mut()) {
            for t in c.take_dvalue_cmds() {
                if let Some(outs) = h.dvalues.command(&t) {
                    c.dvalue_feedback(&mut self.gui, outs);
                }
            }
            // the original saves on a timer (`SlotConfigSaveTimer`); here a change writes the files at once
            if !h.dvalues.take_changed().is_empty() {
                h.dvalues.save_user();
                config_saved = true;
            }
        }
        if config_saved { self.save_viewer_cache(); }
        // `/waypoint` (map marker, `GlobalSignals+0x158`), heard voice messages (`PlayPlayerFX`), `/macro` (docs/chat/dialogs.md §6)
        if let (Some(c), Some(h)) = (self.chat.as_mut(), self.hud.as_ref()) {
            let d = &h.dvalues;
            c.set_voice_prefs(super::chat::VoicePrefs {
                fx_type: d.get_i64("VoiceSndFxType").unwrap_or(0) as i32,
                hear_vicinity: d.flag("VoiceSndFxHearVicinityOn"),
                hear_guild: d.flag("VoiceSndFxHearGuildOn"),
                hear_team: d.flag("VoiceSndFxHearTeamOn"),
            });
        }
        if let (Some(c), Some(h)) = (self.chat.as_mut(), self.hud.as_mut()) {
            c.set_prefs(&mut self.gui, &mut h.dvalues);
            if self.screen == Screen::InWorld {
                c.afk_frame(&mut self.gui, dt, &self.zone, &self.text, self.player.as_ref().is_some_and(|p| p.mode() == 6));
            }
        }
        if let Some(rq) = self.chat.as_mut().map(|c| c.take_requests()) {
            if let (Some(w), Some(h)) = (rq.waypoint, self.hud.as_mut()) {
                h.set_mission(Some(w), None, self.zone.playfield);
            }
            if let (Some(m), Some(h)) = (&rq.macro_drag, self.hud.as_mut()) {
                h.begin_macro_drag(&mut self.gui, m.id, &m.name, &m.command);
            }
            if let Some(a) = &self.audio {
                // `SetMasterPlayerFXMute` / `SetMasterPlayerFXVolume` (`VoiceSndFxOn` / `VoiceSndFxVolume`, SoundOptionsMonitor_c 0x100c487a / 0x100c489b)
                let gain = 1.0;
                for s in &rq.sounds {
                    a.play_sfx(s, gain);
                }
            }
        }
        // Friends / Team Search windows follow the HUD's `friends_window` / `lft_window` dvalues (docs/chat/social.md)
        if let (Some(c), Some(h)) = (self.chat.as_mut(), self.hud.as_mut()) {
            c.sync_windows(&mut self.gui, h.dvalue("friends_window"), h.dvalue("lft_window"), h.dvalue("esc_lft"), h.lft_invite_blocked(), &self.text);
            for (key, window) in c.dock_windows() {
                if let Err(e) = h.register_dock(&mut self.gui, key, window) {
                    eprintln!("dock {key}: {e:#}");
                }
            }
            for d in c.take_closed_windows() {
                h.set_dvalue(&mut self.gui, d, false);
            }
        }
        if let Some(h) = self.hud.as_mut() {
            h.resize(&mut self.gui, size);
            h.set_attack_range(super::hud_target::in_attack_range(&self.zone));
            h.set_vehicle_motion_context(self.dungeon, self.player.as_ref().map(super::player::Player::falling_enabled));
            h.update(&mut self.gui, &mut self.zone, dt);
            if let Some(s) = &self.session {
                for f in h.take_outbox() {
                    if let Some(interact) = self.interact.as_mut() {
                        interact.observe_use_request(&f, &self.zone);
                    }
                    s.send_zone(f);
                }
            }
        }
        // the NCU / team windows' System-window lines, and the Shift + click character info page (`ShowURL("charid://50000/<id>")`)
        if let (Some(h), Some(c)) = (self.hud.as_mut(), self.chat.as_mut()) {
            if h.take_create_reference() {
                c.create_character_reference(&self.zone);
            }
            for (identity, html) in c.take_character_references() {
                h.add_character_reference(identity.kind as u32, identity.instance as u32, html);
            }
            if let Some(key) = h.take_equipment_feedback() {
                c.feedback(&mut self.gui, key, &self.text);
            }
            for l in h.take_system_lines() {
                c.system_line(&mut self.gui, &l, 12);
            }
            for key in h.take_nano_feedback() {
                c.feedback(&mut self.gui, key, &self.text);
            }
            for (line, color) in h.take_waypoint_lines() {
                c.system_line(&mut self.gui, &line, color);
            }
            for n in h.take_casts() {
                c.cast_nano(&mut self.gui, &self.zone, &self.text, n);
            }
            if let Some(id) = h.take_info() {
                c.show_url(&mut self.gui, &self.zone, &self.text, &format!("charid://50000/{id}"));
            }
            for u in h.take_info_urls() {
                c.show_url(&mut self.gui, &self.zone, &self.text, &u);
            }
        }
        self.hud_uses();
        self.interact_frame();
        let (pre, post) = if self.screen == Screen::Create { self.create_frame(dt, host) } else { Default::default() };
        let mut list = self.gui.frame(dt);
        host.hide_cursor = false;
        if self.screen == Screen::InWorld {
            let own = self.zone.own().map_or(host.camera.pos.to_array(), |d| zone::scene_pos(d.pos));
            let indicators = super::tags::indicators(&self.zone);
            let battle_side = self.zone.playfield.filter(|pf| (0x111c..0x1125).contains(pf))
                .map(|_| self.zone.skill_value(0x29c).or_else(|| self.zone.stat_of(self.zone.char_id as i32, 0x29c)).unwrap_or(2)); // GC1005cb6a.
            let stats = &self.zone.character_stats;
            let org_names = &self.zone.org_names;
            let npc_state = &self.zone.tip_npc_state;
            self.zone.world.name_tags(dt, &mut self.gui, host, own, &indicators,
                |id| battle_side.filter(|_| !npc_state.get(&id).copied().unwrap_or(false))
                    .map(|own| (own, stats.get(&id).and_then(|s| s.get(&0x29c)).copied().unwrap_or(2))),
                |id| {
                    stats.get(&id).and_then(|s| s.get(&5)).and_then(|org| org_names.get(org)).cloned()
                });
            self.fight_draw(host, &mut list);
            if let Some(h) = self.hud.as_mut() {
                let (mx, my) = h.mouse_pos();
                let extra_item_hover = self.interact.as_ref().is_some_and(|i| i.icon_hover_at(&self.gui, mx, my));
                h.draw_cursor(&self.gui, &self.zone, self.player.as_ref(), host, &mut list, extra_item_hover);
            }
            self.interact_pointer(host, &mut list);
        }
        if self.screen == Screen::Create {
            list.cmds.splice(0..0, pre.cmds);
            list.cmds.extend(post.cmds);
            if self.cc.as_ref().is_some_and(|c| c.exit_done()) {
                self.cc_close_windows();
                self.cc = None;
                self.start_loading(host);
            }
        }
        let fading_out = matches!(self.fade, Fade::Out(_)) && self.screen == Screen::InWorld;
        if self.screen == Screen::Loading || self.loading_active || fading_out {
            self.loading_overlay(&mut list);
        }
        if self.awaiting_alive {
            // `DisplaySystem+0x44 == 0`: the 3D world is not drawn, the GUI is (black behind it: [INFERENCE])
            let dst = [0.0, 0.0, self.size.0 as f32, self.size.1 as f32];
            list.cmds.splice(0..0, [DrawCmd::Clip(None), DrawCmd::Solid { dst, color: [0, 0, 0], alpha: 1.0 }]);
        }
        list
    }
}


/// `ERRORURL` of `cd_image/data/launcher/AnarchyLauncher.url` (`KEY=value` lines, `#` comments, keys case-insensitive).
fn errorurl(dir: &std::path::Path) -> Option<String> {
    let t = std::fs::read_to_string(dir.join("cd_image/data/launcher/AnarchyLauncher.url")).ok()?;
    t.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("errorurl"))
        .map(|(_, v)| v.trim().to_string())
}

fn log_nano_sound(duration: f32, voices: &[u64]) -> bool {
    duration <= 0.0 || voices.is_empty()
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod live;
