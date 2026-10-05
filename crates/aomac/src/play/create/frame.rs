//! `CharCreateModule_t::FrameProcess` (GUI 0x1011c02c): the module state machine, the intro / exit cinematics, the 3D scene
//! composition and the per-frame drawing.

use super::*;

/// `StaticDoneTimerCallback` ids (GUI 0x1011c9ea).
const CB_DOCKING: u8 = 3;
const CB_BOARDING: u8 = 4;

impl Play {
    /// One frame of the module; returns the draw lists `(below the GUI windows, above them)`.
    pub(in crate::play) fn create_frame(&mut self, dt: f32, host: &mut Host) -> (DrawList, DrawList) {
        let Some(mut c) = self.cc.take() else { return Default::default() };
        let out = self.cc_frame(&mut c, dt, host);
        if c.st == St::Gone {
            self.cc = Some(c);
            self.cc_quit(host);
        } else {
            self.cc = Some(c);
        }
        out
    }

    fn cc_frame(&mut self, c: &mut Create, dt: f32, host: &mut Host) -> (DrawList, DrawList) {
        let (mut pre, mut post) = (DrawList::default(), DrawList::default());
        if c.world.is_none() || c.actors.is_empty() {
            // meshes still loading: the original shows nothing yet
            post.cmds.push(DrawCmd::Solid { dst: [0.0, 0.0, self.size.0 as f32, self.size.1 as f32], color: [0, 0, 0], alpha: 1.0 });
            return (pre, post);
        }
        c.rig.update(dt);
        self.cc_state(c, dt);
        self.cc_timers(c, dt);
        for a in &mut c.actors {
            a.advance(dt);
        }
        if let Some(a) = &self.audio {
            if c.ambience {
                a.play_ui_keepalive("SM_Sandy_CC_Ambience"); // PlaySample every frame (keep-alive)
            }
        }
        self.cc_compose(c, host);
        // camera: `VisualCamera_t::SetPosition/SetRotation/SetViewPlaneWindow(degFOV · π/180 / 45 · …)`
        let pose = c.rig.pose();
        let f = pose.forward();
        let eye = ao_to_render(pose.pos);
        let at = ao_to_render([pose.pos[0] + f[0], pose.pos[1] + f[1], pose.pos[2] + f[2]]);
        host.camera = Camera::look_at(Vec3::from(eye), Vec3::from(at));
        host.lens = Some(Lens { fov: pose.fov_deg.to_radians(), horizontal: true, near: CC_NEAR, far: Some(CC_FAR) });
        self.cc_draw(c, dt, &mut pre, &mut post);
        (pre, post)
    }

    /// The state switch of `FrameProcess`.
    fn cc_state(&mut self, c: &mut Create, dt: f32) {
        // the module's own character jumps to its connector halfway through the camera move (`FUN_10114ec1() >= 0.5`)
        if let St::Wait(sc) = c.st {
            if c.rig.progress() >= 0.5 || c.rig.done() {
                let name = match sc {
                    Sc::Breed => Some("character_0"),
                    Sc::Appearance => Some("character_1"),
                    Sc::Profession => None,
                    Sc::Name => Some("character_3"),
                };
                if let (Some(n), Some(w)) = (name, c.world.as_ref()) {
                    if let (Some(m), Some(a)) = (w.connectors.get(n), c.actors.first_mut()) {
                        if a.place != Some(*m) {
                            a.place = Some(*m);
                            c.scene_dirty = true;
                        }
                    }
                }
                let (bv, pv) = (sc == Sc::Breed, sc == Sc::Profession);
                if (c.breed_vis, c.prof_vis) != (bv, pv) {
                    (c.breed_vis, c.prof_vis) = (bv, pv);
                    c.scene_dirty = true;
                }
            }
        }
        match c.st {
            St::Intro => {
                c.prof_vis = false; // this[0xaa] = 0
                c.scene_dirty = true;
                c.texts_shown = false;
                c.fading = false;
                c.ambience = false;
                c.rig.start(&[5, 1]);
                c.opening_music = self.cc_sound("SM_Sandy_CC_Opening_Music");
                c.fades.push(Fade { t: 0.0, dur: 10.0, a0: 1.0, a1: 0.0, done: None }); // PlainSpriteFade_t(10, mode 1)
                c.st = St::IntroWait;
            }
            St::IntroWait => {
                if c.rig.done() {
                    c.rig.start(&[1, 1, 1]);
                    c.fades.push(Fade { t: 0.0, dur: 4.0, a0: 1.0, a1: 0.0, done: Some(CB_DOCKING) });
                    c.st = St::Pick(Sc::Breed);
                } else {
                    let dur = c.rig.duration();
                    let elapsed = c.rig.progress() * dur;
                    let remaining = dur - elapsed;
                    if elapsed <= 4.0 || c.texts_shown {
                        if remaining < 3.0 && !c.fading {
                            c.fades.push(Fade { t: 0.0, dur: remaining + 0.02, a0: 0.0, a1: 1.0, done: None });
                            c.fading = true;
                        }
                    } else {
                        let (a, b) = (self.cc_text("MorningStar"), self.cc_text("GeosynchronousOrbit"));
                        c.text_fades.push(TextFade { t: 0.0, fade: 5.0, hold: 4.0, text: a, pos: (0.5, 0.9) });
                        c.text_fades.push(TextFade { t: 0.0, fade: 5.0, hold: 4.0, text: b, pos: (0.5, 0.925) });
                        c.texts_shown = true;
                    }
                }
            }
            St::Pick(sc) => {
                c.cur = Some(sc);
                if sc == Sc::Profession {
                    c.prof_vis = true; // this[0xaa] = 1
                }
                c.st = St::Wait(sc);
            }
            St::Wait(sc) => {
                if c.rig.done() {
                    c.ambience = true;
                    self.cc_start_scene(c, sc);
                    c.st = St::Active;
                }
            }
            St::Active => {
                if let Some((t, code)) = c.stop {
                    let t = t - dt;
                    if t > 0.0 {
                        c.stop = Some((t, code));
                    } else {
                        c.stop = None;
                        self.cc_scene_done(c, code);
                    }
                }
            }
            St::Exit => {
                c.exit_t += dt;
                if c.handoff && c.exit_t >= 1.0 && !c.created {
                    c.created = true;
                }
            }
            St::Gone => {}
        }
    }

    /// `FrameProcess` after `scene->FrameProcess()` returned 0x65 / 0x66: the next / previous scene.
    fn cc_scene_done(&mut self, c: &mut Create, code: i32) {
        let Some(cur) = c.cur else { return };
        let i = cur.index() as i32;
        if code == 0x65 {
            if i + 1 > 3 {
                self.cc_begin_exit(c);
            } else {
                c.st = St::Pick(Sc::from(i as usize + 1));
            }
        } else {
            c.st = St::Pick(Sc::from((i - 1).max(0) as usize));
        }
    }

    /// State 0x4b1: the scene is gone, the shuttle flies off ([4 3]); after 1 s the boarding text and the loading screen.
    pub(super) fn cc_begin_exit(&mut self, c: &mut Create) {
        c.cur = None;
        c.st = St::Exit;
        c.exit_t = 0.0;
        self.cc_hide_ui(c);
        c.rig.start(&[4, 3]);
        c.timers.push(Timer { left: 1.0, id: CB_BOARDING });
    }

    fn cc_timers(&mut self, c: &mut Create, dt: f32) {
        let mut fired = vec![];
        for f in &mut c.fades {
            f.t += dt;
            if f.t >= f.dur {
                fired.extend(f.done.take());
            }
        }
        c.fades.retain(|f| f.t < f.dur);
        for t in &mut c.timers {
            t.left -= dt;
            if t.left <= 0.0 {
                fired.push(t.id);
            }
        }
        c.timers.retain(|t| t.left > 0.0);
        for t in &mut c.text_fades {
            t.t += dt;
        }
        c.text_fades.retain(|t| t.t < 2.0 * t.fade + t.hold);
        for id in fired {
            self.cc_callback(c, id);
        }
    }

    /// `CharCreateModule_t::StaticDoneTimerCallback` (GUI 0x1011c9ea).
    fn cc_callback(&mut self, c: &mut Create, id: u8) {
        if c.cur.is_some() && c.rig.done() {
            return;
        }
        match id {
            CB_DOCKING => {
                let (a, b) = (self.cc_text("DockingCompleted"), self.cc_text("ContinuingDNA"));
                c.text_fades.push(TextFade { t: 0.0, fade: 6.0, hold: 4.0, text: a, pos: (0.5, 0.9) });
                c.text_fades.push(TextFade { t: 0.0, fade: 6.0, hold: 4.0, text: b, pos: (0.5, 0.925) });
            }
            CB_BOARDING => {
                let a = self.cc_text("BoardingShuttle");
                c.text_fades.push(TextFade { t: 0.0, fade: 1.0, hold: 1.0, text: a, pos: (0.5, 0.9) });
                c.created = true; // AFCM::Send(0x12, 0x3d): the loading program takes over
            }
            _ => {}
        }
    }

    // ---- 3D scene ----------------------------------------------------------------------------------------------

    fn cc_compose(&mut self, c: &mut Create, host: &mut Host) {
        let Some(world) = c.world.as_ref() else { return };
        let main_visible = c.breed != 0 || c.cur.is_some_and(|s| s != Sc::Breed);
        let vis: Vec<bool> = c.actors.iter().enumerate().map(|(i, a)| a.first.is_some() && a.place.is_some() && if i == 0 { main_visible } else { c.breed_vis }).collect();
        let full = c.scene_dirty || c.actors.iter().any(|a| a.fresh) || c.uploaded != vis;
        let mut scene = Scene::default();
        if full {
            scene = world.scene.clone();
            scene.instances.clear();
        } else {
            scene.meshes = vec![Mesh::default(); world.scene.meshes.len()];
        }
        for (slot, inst) in world.scene.instances.iter().enumerate() {
            if !PROFESSION_MESHES.contains(&slot) || c.prof_vis {
                scene.instances.push(*inst);
            }
        }
        for (i, a) in c.actors.iter_mut().enumerate() {
            a.fresh = false;
            let Some(first) = a.first.as_deref() else { continue };
            let base = scene.meshes.len();
            if !vis[i] {
                scene.meshes.extend(std::iter::repeat_n(Mesh::default(), first.meshes.len()));
                continue;
            }
            let src = a.cur.as_ref().unwrap_or(first);
            if full {
                scene.textures.extend(first.textures.iter().map(|(k, t)| (*k, t.clone())));
            }
            scene.meshes.extend(src.meshes.iter().cloned());
            for inst in &src.instances {
                if let Some(t) = a.transform(&inst.transform) {
                    scene.instances.push(ao_scene::Instance { mesh: inst.mesh + base, transform: t });
                }
            }
        }
        if full {
            c.uploaded = vis;
            c.scene_dirty = false;
            host.set_scene(scene);
        } else {
            host.repose(scene);
        }
    }

    // ---- drawing -----------------------------------------------------------------------------------------------

    fn cc_draw(&mut self, c: &mut Create, _dt: f32, pre: &mut DrawList, post: &mut DrawList) {
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        if c.shown && c.cur.is_some() {
            // text area surface (`ViewSurface_c` colour 0x6666aa, alpha 0.7) under the framed text view
            let r = c.info_rect;
            pre.cmds.push(DrawCmd::Solid { dst: [r[0], r[1], r[0] + r[2], r[1] + r[3]], color: [0x66, 0x66, 0xaa], alpha: 0.7 });
            let font = FontId::Cc17;
            let fh = self.gui.font_height(font);
            let mut lbls = vec![(c.banner.clone(), 0.5, 0.07)];
            lbls.extend(c.labels.iter().cloned());
            for (text, fx, fy) in lbls {
                if text.is_empty() {
                    continue;
                }
                let lines: Vec<&str> = text.split('\n').map(|l| l.trim_end_matches('\r')).collect();
                let tw = lines.iter().map(|l| self.gui.text_width(font, l)).max().unwrap_or(0);
                let (lw, lh) = (tw as f32 + 10.0, (fh * lines.len() as i32) as f32 + 2.0);
                let (x0, y0) = ((w * fx - lw * 0.5).floor(), (h * fy - lh * 0.5).floor());
                pre.cmds.push(DrawCmd::Solid { dst: [x0, y0, x0 + lw, y0 + lh], color: [0x66, 0x66, 0xaa], alpha: 0.703125 });
                for (i, l) in lines.iter().enumerate() {
                    let (tx, ty) = (x0 as i32 + 5, y0 as i32 + 1 + fh * i as i32);
                    self.gui.text_cmds(font, l, tx + 1, ty + 1, 0x000000, 1.0, post); // shadow offset (1,1)
                    self.gui.text_cmds(font, l, tx, ty, 0xffffff, 1.0, post);
                }
            }
            for (i, b) in c.btns.iter().enumerate() {
                if b.invisible {
                    continue;
                }
                let state = if !b.enabled {
                    0
                } else if c.pressed == Some(i) && c.hover == Some(i) {
                    1
                } else if c.hover == Some(i) {
                    2
                } else {
                    0
                };
                let id = b.gfx[state];
                let (iw, ih) = self.gui.gfx().size(id);
                let tint = if b.enabled { 255 } else { 0x90 };
                post.cmds.push(DrawCmd::Gfx { id, src: [0.0, 0.0, iw as f32, ih as f32], dst: [b.rect[0], b.rect[1], b.rect[0] + b.rect[2], b.rect[1] + b.rect[3]], tint: [tint; 3], alpha: 1.0 });
            }
        }
        for t in &c.text_fades {
            let a = if t.t < t.fade {
                t.t / t.fade
            } else if t.t >= t.fade + t.hold {
                1.0 - (t.t - t.fade - t.hold) / t.fade
            } else {
                1.0
            }
            .clamp(0.0, 1.0);
            let tw = self.gui.text_width(FontId::Cc17, &t.text);
            let fh = self.gui.font_height(FontId::Cc17);
            let (x, y) = ((w * t.pos.0 - tw as f32 * 0.5).floor() as i32, (h * t.pos.1 - fh as f32 * 0.5).floor() as i32);
            self.gui.text_cmds(FontId::Cc17, &t.text, x, y, 0xffffff, a, post);
        }
        for f in &c.fades {
            let p = (f.t / f.dur).clamp(0.0, 1.0);
            let a = f.a0 + (f.a1 - f.a0) * p;
            post.cmds.push(DrawCmd::Solid { dst: [0.0, 0.0, w, h], color: [0, 0, 0], alpha: a });
        }
    }
}
