//! The four scenes (`BreedScene_t`, `AppearanceScene_t`, `ProfessionScene_t`, `NameScene_t`) and their `SceneBase_t`:
//! widgets, texts, hover / click behaviour, camera moves, the name entry and the network replies.

use super::*;

/// The framed text area: `BorderView_c::SetGfx(0xc1,0xc5,0xba,0xbc,0xbe,0xc3,0xbf,0xbb)` + `SetLocalAlpha(0.7)`, a `TextView_c`
/// (font TT_MIN12, flags 0x60, borders 5,2,5,5) inside; the frame is set to the text-area rectangle.
fn info_xml(w: u32, h: u32) -> String {
    format!(
        r##"<?xml version="1.0" standalone="yes"?>
<root><View view_layout="vertical">
<BorderView name="frame" tl_gfx="GFX_GUI_DIALOG2_TL_SIMPLE" tr_gfx="GFX_GUI_DIALOG2_TR_SIMPLE" bl_gfx="GFX_GUI_DIALOG2_BL" br_gfx="GFX_GUI_DIALOG2_BR"
 left_gfx="GFX_GUI_DIALOG2_LEFT" top_gfx="GFX_GUI_DIALOG2_TOP_SIMPLE" right_gfx="GFX_GUI_DIALOG2_RIGHT" bottom_gfx="GFX_GUI_DIALOG2_BOTTOM" alpha="0.7"
 min_size="Point({w},{h})" max_size="Point({w},{h})" v_alignment="TOP" h_alignment="LEFT">
<TextView name="info" value="" font="TT_MIN12" feature_flags="TVF_MULTILINE|TVF_WORD_WRAP" layout_borders="Rect(5,2,5,5)"/>
<VLayoutSpacer/>
</BorderView></View></root>"##
    )
}

const NAME_XML: &str = r##"<?xml version="1.0" standalone="yes"?>
<root><View view_layout="horizontal">
<TextView name="label" value="Nickname:" font="CC17" color="0xffffff" layout_borders="Rect(0,0,5,0)"/>
<TextInputView name="name" value="" font="CC17" color="0xaaaaff" min_size="Point(200,-1)"/>
</View></root>"##;

/// `GFX_GUI_CC_NEW_PROF<k>` ids of the 14 profession buttons (all three states use the same image).
const PROF_GFX: [u32; 14] = [0x5e, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x6b, 0x5f, 0x60, 0x61, 0x62, 0x63];

fn rect_in(b: &Btn, p: (f32, f32)) -> bool {
    p.0 >= b.rect[0] && p.0 < b.rect[0] + b.rect[2] && p.1 >= b.rect[1] && p.1 < b.rect[1] + b.rect[3]
}

impl Play {
    fn cc_btn(&mut self, kind: B, ids: [u32; 3], fx: f32, fy: f32) -> Btn {
        let (iw, ih) = self.gui.gfx().size(GfxId(ids[0]));
        let (w, h) = (self.size.0 as f32 / 1024.0 * iw as f32, self.size.1 as f32 / 768.0 * ih as f32);
        // `SceneBase_t::MakeButton`: centred on (fx·W, fy·H), floored
        let (x, y) = ((self.size.0 as f32 * fx - w * 0.5).floor(), (self.size.1 as f32 * fy - h * 0.5).floor());
        Btn { kind, gfx: ids.map(GfxId), rect: [x, y, w, h], enabled: true, invisible: false }
    }

    fn cc_hot(&self, kind: B, rect: [f32; 4]) -> Btn {
        Btn { kind, gfx: [GfxId(0); 3], rect, enabled: true, invisible: true }
    }

    pub(super) fn cc_hide_ui(&mut self, c: &mut Create) {
        c.shown = false;
        c.btns.clear();
        c.hover = None;
        c.pressed = None;
        for w in [c.info_w.take(), c.name_w.take()].into_iter().flatten() {
            self.gui.close_window(w);
        }
    }

    /// `SceneBase_t::SetNewInfoText(title, body)`.
    fn cc_info(&mut self, c: &mut Create, title: &str, body: &str) {
        c.info = (title.to_string(), body.to_string());
        let html = format!("<DIV align=\"left\"><FONT color=\"#eeeeee\">{title}\n</FONT><FONT color=\"#cccccc\">{body}</FONT></DIV>");
        if let Some(w) = c.info_w {
            self.gui.set_text(w, "info", &html);
        }
    }

    fn cc_info_keys(&mut self, c: &mut Create, title: &str, body: &str) {
        let (t, b) = (self.cc_text(title), self.cc_text(body));
        self.cc_info(c, &t, &b);
    }

    fn cc_prof_texts(&self, k: i32) -> (String, String, String) {
        let key = PROF_KEYS[(k - 1) as usize];
        let desc = if k == 1 { "DescriptionMetaPhysicists".to_string() } else { format!("Description{key}") };
        (self.cc_text(&format!("Inspect{key}")), self.cc_text(&format!("{key}Selected")), self.cc_text(&desc))
    }

    fn cc_breed_key(i: usize) -> (&'static str, &'static str, &'static str) {
        // (hover title key, description key, `…Selected` key) per CC breed 1..7
        [
            ("ButtonSolitusFemale", "SolitusDescription", "SolitusFemaleSelected"),
            ("ButtonSolitusMale", "SolitusDescription", "SolitusMaleSelected"),
            ("ButtonOpifexFemale", "OpifexDescription", "OpifexFemaleSelected"),
            ("ButtonOpifexMale", "OpifexDescription", "OpifexMaleSelected"),
            ("ButtonNanomageFemale", "NanomageDescription", "NanomageFemaleSelected"),
            ("ButtonNanomageMale", "NanomageDescription", "NanomageMaleSelected"),
            ("ButtonAtrox", "AtroxDescription", "AtroxSelected"),
        ][i]
    }

    /// `SetBackgroundText` of the current scene: what the text area shows when nothing is hovered.
    pub(super) fn cc_background_text(&mut self, c: &mut Create) {
        match c.cur {
            Some(Sc::Breed) => {
                if c.breed == 0 {
                    self.cc_info_keys(c, "ChooseBreed", "BreedHelp");
                } else {
                    let (_, d, s) = Self::cc_breed_key(c.breed as usize - 1);
                    self.cc_info_keys(c, s, d);
                }
            }
            Some(Sc::Appearance) => {
                self.cc_info_keys(c, "SelectAppearance", "SelectionHelpText");
                self.cc_full_view(c);
            }
            Some(Sc::Profession) => {
                if c.prof == 0 {
                    self.cc_info_keys(c, "ProfButtonDefault", "ProfessionDefault");
                } else {
                    let (_, sel, desc) = self.cc_prof_texts(c.prof);
                    self.cc_info(c, &sel, &desc);
                }
            }
            Some(Sc::Name) => self.cc_info_keys(c, "ChooseNick", "NickHelp"),
            None => {}
        }
    }

    /// `SceneBase_t::StartScene`: Activate (build + show the widgets) and `SetBackgroundText`.
    pub(super) fn cc_start_scene(&mut self, c: &mut Create, sc: Sc) {
        self.cc_hide_ui(c);
        c.cur = Some(sc);
        c.shown = true;
        c.stop = None;
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        let (title, rect, centre) = match sc {
            Sc::Breed => ("BannerSelectBreed", ((w * 0.55).floor(), (h * 0.25).floor()), (0.575, 0.85)),
            Sc::Appearance => ("BannerSelectAppearance", ((w * 0.5).floor(), (h * 0.2).floor()), (0.68, 0.77)),
            Sc::Profession => ("BannerSelectProfession", ((w * 0.6).floor(), (((h / 768.0 - 1.0) * 0.4 + 1.0) * 156.0).floor()), (0.325, 0.8125)),
            Sc::Name => ("BannerEnterName", ((w * 0.6).floor(), (h * 0.25).floor()), (0.68, 0.77)),
        };
        c.banner = self.cc_text(title);
        c.labels.clear();
        c.info_rect = [(w * centre.0 - rect.0 * 0.5).floor(), (h * centre.1 - rect.1 * 0.5).floor(), rect.0, rect.1];
        // the framed HTML text area
        if let Ok(win) = self.gui.open_window_xml("cc_info", &info_xml(rect.0 as u32, rect.1 as u32), (c.info_rect[0] as i32, c.info_rect[1] as i32), WindowSize::Fixed(rect.0 as u32, rect.1 as u32)) {
            c.info_w = Some(win);
        }
        // common buttons: exit, next, back (z-order bottom → top)
        let exit = self.cc_btn(B::Exit, [0x52, 0x54, 0x53], 0.975, 0.05);
        let next = self.cc_btn(B::Next, [0x6f, 0x71, 0x70], 0.925, 0.95);
        let back = self.cc_btn(B::Back, [0x4f, 0x51, 0x50], if sc == Sc::Name { 0.774609 } else { 0.8 }, 0.95);
        c.btns.push(exit);
        if sc != Sc::Name {
            c.btns.push(next);
        }
        if sc != Sc::Breed {
            c.btns.push(back);
        }
        match sc {
            Sc::Breed => {
                // StopAllSoundEffects, then SM_Sandy_CC_Select_Breed
                if let Some(a) = &self.audio {
                    for id in c.opening_music.drain(..) {
                        a.stop(id);
                    }
                }
                self.cc_sound("SM_Sandy_CC_Select_Breed");
                c.btns.insert(0, self.cc_hot(B::BreedBg, [0.0, 0.0, w, h]));
                if c.breed == 0 {
                    if let Some(b) = c.btns.iter_mut().find(|b| b.kind == B::Next) {
                        b.enabled = false;
                    }
                }
                c.hover_breed = 0;
                c.last_hover_breed = 0;
                self.cc_breed_label(c);
            }
            Sc::Appearance => {
                self.cc_sound("SM_Sandy_CC_Appearance");
                for (key, fx, fy) in [("SelectHead", 0.25, 0.17), ("SelectHeight", 0.79, 0.2), ("SelectBuild", 0.79, 0.5)] {
                    let t = self.cc_text(key);
                    c.labels.push((t, fx, fy));
                }
                for (kind, ids, fx, fy) in [
                    (B::Height(2), [0x83, 0x85, 0x84], 0.9, 0.28),
                    (B::Height(1), [0x58, 0x5a, 0x59], 0.79, 0.28),
                    (B::Height(0), [0x7a, 0x7c, 0x7b], 0.68, 0.28),
                    (B::Build(2), [0x55, 0x57, 0x56], 0.9, 0.58),
                    (B::Build(1), [0x58, 0x5a, 0x59], 0.79, 0.58),
                    (B::Build(0), [0x80, 0x82, 0x81], 0.68, 0.58),
                    (B::HeadNext, [0x6c, 0x6e, 0x6d], 0.4, 0.17),
                    (B::HeadPrev, [0x74, 0x76, 0x75], 0.1, 0.17),
                ] {
                    let b = self.cc_btn(kind, ids, fx, fy);
                    c.btns.push(b);
                }
                // invisible head hot zone (floor(0.48 W) × floor(0.3 W) around (0.23 W, 0.3 H)), behind everything
                let (hw, hh) = ((w * 0.48).floor(), (w * 0.3).floor());
                let area = self.cc_hot(B::HeadArea, [(w * 0.23 - hw * 0.5).floor(), (h * 0.3 - hh * 0.5).floor(), hw, hh]);
                c.btns.insert(0, area);
                c.head_state = 0;
            }
            Sc::Profession => {
                self.cc_sound("SM_Sandy_CC_Select_Profession");
                let sl = self.char_list.expansions & 2 != 0 || self.char_list.sl_profs_enabled != 0;
                let mut fy = 0.2;
                for k in 1..=14i32 {
                    let g = PROF_GFX[k as usize - 1];
                    let mut b = self.cc_btn(B::Prof(k), [g, g, g], 0.82, fy);
                    if !sl && (k == 5 || k == 6) {
                        b.enabled = false; // Keeper / Shade need the Shadowlands expansion
                    }
                    let (_, ih) = self.gui.gfx().size(GfxId(g));
                    fy += (ih as f32 + 4.0) / 768.0;
                    c.btns.push(b);
                }
                if c.prof == 0 {
                    if let Some(b) = c.btns.iter_mut().find(|b| b.kind == B::Next) {
                        b.enabled = false;
                    }
                }
            }
            Sc::Name => {
                self.cc_sound("SM_Sandy_CC_Name_Character");
                let suggest = self.cc_btn(B::Suggest, [0x5b, 0x5d, 0x5c], 0.5, 0.23);
                let finish = self.cc_btn(B::Finish, [0x77, 0x79, 0x78], 0.912305, 0.95);
                c.btns.push(suggest);
                c.btns.push(finish);
                c.name_locked = false;
                if let Ok(win) = self.gui.open_window_xml("cc_name", NAME_XML, (0, 0), WindowSize::Preferred) {
                    let lbl = self.cc_text("Nickname:");
                    self.gui.set_text(win, "label", &lbl);
                    let (ow, oh) = self.gui.outer_size(win);
                    self.gui.set_window_pos(win, ((w * 0.5 - ow as f32 * 0.5).floor() as i32, (h * 0.17 - oh as f32 * 0.5).floor() as i32));
                    self.gui.focus(win, "name");
                    c.name_w = Some(win);
                }
                self.cc_name_info(c);
                self.cc_update_finish(c);
            }
        }
        self.cc_background_text(c);
    }

    fn cc_breed_label(&mut self, c: &mut Create) {
        c.labels.clear();
        if c.breed != 0 {
            if let Some((b, s)) = cc_breed_to_gc(c.breed) {
                c.labels.push((format!("{} {}", super::flow::breed_name(b), super::flow::sex_name(s)), 0.82, 0.17));
            }
        }
    }

    /// `NameScene_t::CreateCharInfo`: "Breed: %s\r\nProfession: %s" at (0.24, 0.95).
    fn cc_name_info(&mut self, c: &mut Create) {
        let (b, _) = cc_breed_to_gc(c.breed).unwrap_or((1, 3));
        let prof = self.text.by_id(2004, cc_prof_to_gc(c.prof) as u32).unwrap_or_default();
        let fmt = self.cc_text("BreedProfession");
        let t = fmt.replacen("%s", super::flow::breed_name(b), 1).replacen("%s", &prof, 1);
        c.labels.retain(|l| !l.0.starts_with(fmt.split("%s").next().unwrap_or("Breed")));
        c.labels.push((t, 0.24, 0.95));
    }

    /// `NameScene_t::UpdateButtonState`: Finish only with a non-empty name.
    fn cc_update_finish(&mut self, c: &mut Create) {
        let non_empty = c.name_w.is_some_and(|w| !self.gui.text(w, "name").is_empty());
        if let Some(b) = c.btns.iter_mut().find(|b| b.kind == B::Finish) {
            b.enabled = non_empty && !c.name_locked;
        }
    }

    // ---- camera moves ------------------------------------------------------------------------------------------

    fn cc_snap(&mut self, c: &mut Create, id: &[u32]) {
        let p = c.rig.pose();
        c.rig.set_camera(id, p); // `FUN_10115903(id, camera pos, rot, fov)`: the current view becomes camera `id`
    }

    /// `AppearanceScene_t::MoveCameraToFullView` (GUI 0x10112600).
    fn cc_full_view(&mut self, c: &mut Create) {
        if c.head_state != 0 && c.head_state != 2 {
            if let Some(a) = c.actors.first_mut() {
                a.play_idle(false);
            }
            self.cc_snap(c, &[2, 3]);
            c.rig.set_transition(&[2, 1, 3], 0.3, vec![vec![2, 3], vec![2, 1]]);
            c.rig.start(&[2, 1, 3]);
            c.head_state = 2;
        }
    }

    /// `AppearanceScene_t::MoveCamereToHead` (GUI 0x1011231d): camera [2 2] is re-aimed at the head attractor.
    fn cc_head_view(&mut self, c: &mut Create) {
        if c.head_state == 1 || c.head_state == 4 {
            return;
        }
        if let Some(a) = c.actors.first_mut() {
            a.play_idle(true);
        }
        self.cc_snap(c, &[2, 3]);
        if let Some(a) = c.actors.first() {
            // head attractor world position: mount of the head mesh (the second instance of the character scene)
            if let (Some(f), Some(cam)) = (a.first.as_deref(), c.rig.camera(&[2, 2])) {
                if let Some(head) = f.instances.get(1).and_then(|i| a.transform(&i.transform)) {
                    let p = [head[3][0], head[3][1], -head[3][2]];
                    c.rig.set_camera(&[2, 2], ao_formats::create::Pose { pos: [p[0] - 0.081, p[1] - 0.009, p[2] + 1.1112], ..cam });
                }
            }
        }
        c.rig.set_transition(&[2, 1, 3], 0.3, vec![vec![2, 3], vec![2, 2]]);
        c.rig.start(&[2, 1, 3]);
        c.head_state = 4;
    }

    /// `ProfessionScene_t::ProfessionChanged` (GUI 0x10120f55).
    fn cc_prof_changed(&mut self, c: &mut Create, p: i32) {
        let old = c.prof;
        c.prof = p;
        self.prefs.cc.profession = p;
        self.prefs.save();
        if old != p {
            let a = if old < 1 { vec![3, 1] } else { vec![3, 1, old as u32] };
            let b = vec![3, 1, p as u32];
            let m = if p < 7 { p } else { 14 - p } / 2 + 1;
            c.rig.set_transition(&[3, 1, 1], 0.3, vec![a, vec![3, 2, m as u32], b]);
            c.rig.start(&[3, 1, 1]);
            self.cc_sound(PROF_SOUNDS[p as usize - 1]);
        }
        if let Some(b) = c.btns.iter_mut().find(|b| b.kind == B::Next) {
            b.enabled = true;
        }
    }

    fn cc_save_prefs(&mut self, c: &Create) {
        self.prefs.cc = prefs::CcPrefs { breed: c.breed, height: c.height, size: c.size, head: c.head as i32, profession: c.prof };
        self.prefs.save();
    }

    // ---- main character ----------------------------------------------------------------------------------------

    /// Rebuilds the module's character after a breed / head / build change (`CCCharacter_t::SetBreed/SetHeadIndex/SetBuild`).
    fn cc_main_respec(&mut self, c: &mut Create) {
        if c.breed == 0 {
            return;
        }
        let (b, s, _) = CC_BREEDS[c.breed as usize - 1];
        if let Some((breed, gender)) = gc_breed(b, s) {
            let table = head_table(&self.dir, breed, gender);
            if c.heads != table {
                c.heads = table;
            }
        }
        c.head = c.head.min(c.heads.len().saturating_sub(1));
        if let Some(spec) = c.spec(c.breed as usize - 1, c.head, c.size) {
            if let Some(a) = c.actors.first_mut() {
                a.respec(spec);
                a.height = c.height;
            }
        }
    }

    // ---- input -------------------------------------------------------------------------------------------------

    pub(in crate::play) fn create_input(&mut self, ev: &InputEvent, host: &mut Host) -> bool {
        let Some(mut c) = self.cc.take() else { return false };
        let used = self.cc_input(&mut c, ev, host);
        self.cc = Some(c);
        used
    }

    fn cc_hit(&self, c: &Create, p: (f32, f32)) -> Option<usize> {
        c.btns.iter().enumerate().rev().find(|(_, b)| rect_in(b, p)).map(|(i, _)| i)
    }

    fn cc_input(&mut self, c: &mut Create, ev: &InputEvent, _host: &mut Host) -> bool {
        if !c.shown || self.dialog_w.is_some() {
            return false;
        }
        match ev {
            InputEvent::MouseMove { x, y } => {
                c.mouse = (*x, *y);
                let h = self.cc_hit(c, c.mouse);
                self.cc_set_hover(c, h);
                if h.is_some_and(|i| c.btns[i].kind == B::BreedBg) {
                    self.cc_pick_breed(c);
                }
                false
            }
            InputEvent::MouseDown { x, y, button: MouseButton::Left } => {
                c.mouse = (*x, *y);
                let h = self.cc_hit(c, c.mouse);
                c.pressed = h.filter(|&i| c.btns[i].enabled);
                // the name input and the text area live in GUI windows: keep passing the event on
                h.is_some_and(|i| !c.btns[i].invisible)
            }
            InputEvent::MouseUp { x, y, button: MouseButton::Left } => {
                c.mouse = (*x, *y);
                let h = self.cc_hit(c, c.mouse);
                let p = c.pressed.take();
                if let (Some(i), Some(j)) = (h, p) {
                    if i == j && c.btns[i].enabled {
                        let kind = c.btns[i].kind;
                        self.cc_click(c, kind);
                    }
                }
                false
            }
            _ => false,
        }
    }

    /// Hover enter / leave (`MouseTransition_e` 2 / 3) of the widgets.
    fn cc_set_hover(&mut self, c: &mut Create, new: Option<usize>) {
        if c.hover == new {
            return;
        }
        let old = std::mem::replace(&mut c.hover, new);
        if let Some(i) = old {
            if !matches!(c.btns[i].kind, B::BreedBg) {
                self.cc_background_text(c);
            }
        }
        if let Some(i) = new {
            // enabled or not: `Window::_FindView` ignores the enabled flag and `_CallMouseMoved` emits the view's mouse signal
            // before the (virtual) `ButtonBase_c::MouseMove`, so a disabled button still plays its sound and sets its text
            let kind = c.btns[i].kind;
            self.cc_hover_enter(c, kind);
        }
        if new.is_none_or(|i| c.btns[i].kind != B::BreedBg) && c.hover_breed != 0 {
            c.hover_breed = 0;
            c.last_hover_breed = 0;
        }
    }

    fn cc_hover_enter(&mut self, c: &mut Create, kind: B) {
        let sc = c.cur;
        let mouseover = |s: &mut Play| {
            s.cc_sound("SM_Sandy_CC_GUI_Mouseover");
        };
        match kind {
            B::BreedBg => {}
            B::HeadArea => {
                mouseover(self);
                self.cc_info_keys(c, "ButtonSelectFace", "Select Face");
                self.cc_head_view(c);
            }
            B::Exit => {
                mouseover(self);
                self.cc_info_keys(c, "ButtonExit", "SelectionsWillBeSaved");
            }
            B::Next => {
                mouseover(self);
                let body = match sc {
                    Some(Sc::Breed) => "BreedNext",
                    Some(Sc::Appearance) => "ProceedToProfession",
                    Some(Sc::Profession) => "ProfNext",
                    _ => "NameFinish",
                };
                self.cc_info_keys(c, "ButtonNext", body);
            }
            B::Back => {
                mouseover(self);
                let body = match sc {
                    Some(Sc::Appearance) => "BackToBreed",
                    Some(Sc::Profession) => "ProfBack",
                    _ => "NameBack",
                };
                self.cc_info_keys(c, "ButtonGoBack", body);
            }
            B::Height(h) => {
                mouseover(self);
                let (t, b) = [("ButtonShort", "HeightToShort"), ("ButtonMedium", "HeightToMedium"), ("ButtonTall", "HeightToTall")][h as usize];
                self.cc_info_keys(c, t, b);
            }
            B::Build(s) => {
                mouseover(self);
                let (t, b) = [("ButtonSlender", "BodytypeSlender"), ("ButtonBodyMedium", "BodytypeMedium"), ("ButtonHeavy", "BodytypeHeavy")][s as usize];
                self.cc_info_keys(c, t, b);
            }
            B::HeadPrev | B::HeadNext => mouseover(self),
            B::Suggest => {
                mouseover(self);
                self.cc_info_keys(c, "NameButtonSuggest", "NameSuggest");
            }
            B::Finish => {
                mouseover(self);
                self.cc_info_keys(c, "NameButtonFinish", "Finish_AreteLanding");
            }
            B::Prof(k) => {
                mouseover(self);
                let (inspect, _, desc) = self.cc_prof_texts(k);
                self.cc_info(c, &inspect, &desc);
            }
        }
    }

    /// `BreedScene_t::SlotBreedButton`: the breed character under the mouse (ray vs. posed meshes).
    fn cc_pick_breed(&mut self, c: &mut Create) {
        let pose = c.rig.pose();
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        let ndc = (c.mouse.0 / w * 2.0 - 1.0, c.mouse.1 / h * 2.0 - 1.0);
        let tan = (pose.fov_deg.to_radians() * 0.5).tan();
        let aspect = w / h;
        let dir_cam = [tan * ndc.0, -ndc.1 * tan / aspect, 1.0];
        let d = rot_vec(pose.rot, dir_cam);
        let (origin, dir) = (V3::from(ao_to_render(pose.pos)), V3::from(ao_to_render(d)).normalize());
        let mut best: Option<(i32, f32)> = None;
        for (i, a) in c.actors.iter().enumerate().skip(1) {
            let (Some(cur), Some(first)) = (a.cur.as_ref().or(a.first.as_deref()), a.first.as_deref()) else { continue };
            let Some(xf) = a.transform(&ao_scene::IDENTITY) else { continue };
            let _ = first;
            if let Some(t) = actor::ray_hit(cur, &xf, origin, dir) {
                if best.is_none_or(|b| t < b.1) {
                    best = Some((i as i32, t));
                }
            }
        }
        // actors[1..=7] are CC breeds 1..7 (`CharCreateModule + 0x84 + 4·i`); the first hit in index order wins (the loop keeps the last hit)
        let id = best.map_or(0, |b| b.0);
        c.hover_breed = id;
        if id != c.last_hover_breed {
            c.last_hover_breed = id;
            if id == 0 {
                self.cc_background_text(c);
            } else {
                let (t, d, _) = Self::cc_breed_key(id as usize - 1);
                self.cc_info_keys(c, t, d);
            }
            if id != c.breed && id != 0 {
                self.cc_sound("SM_Sandy_CC_GUI_Mouseover"); // ShowSelectionGlow(true) + mouse-over sound
            }
        }
    }

    // ---- clicks ------------------------------------------------------------------------------------------------

    fn cc_click(&mut self, c: &mut Create, kind: B) {
        let select = |s: &mut Play| {
            s.cc_sound("SM_Sandy_CC_GUI_Select");
        };
        match kind {
            B::Exit => {
                select(self);
                self.cc_ask_exit(c);
            }
            B::BreedBg => self.cc_breed_pressed(c),
            B::Next => self.cc_next(c),
            B::Back => self.cc_prev(c),
            B::HeadNext | B::HeadPrev => {
                select(self);
                let n = c.heads.len().max(1);
                // `SelectNextHead` steps back, `SelectPrevHead` forward (GUI 0x10111b2e / 0x10111b04), as the buttons are wired
                c.head = if kind == B::HeadNext { (c.head + n - 1) % n } else { (c.head + 1) % n };
                self.cc_main_respec(c);
                self.cc_head_view(c);
            }
            B::Height(h) => {
                select(self);
                c.height = h;
                if let Some(a) = c.actors.first_mut() {
                    a.height = h;
                }
                self.cc_full_view(c);
            }
            B::Build(s) => {
                select(self);
                c.size = s;
                self.cc_main_respec(c);
                self.cc_full_view(c);
            }
            B::Prof(k) => {
                select(self);
                self.cc_prof_changed(c, k);
            }
            B::Suggest => {
                select(self);
                c.pending_suggest = true;
                self.cc_suggest(c);
            }
            B::Finish => {
                select(self);
                c.btns.iter_mut().filter(|b| matches!(b.kind, B::Finish | B::Back)).for_each(|b| b.enabled = false);
                self.cc_start_creation(c);
            }
            B::HeadArea => {}
        }
    }

    /// `BreedScene_t::SlotBreedPressed` (GUI 0x10113526).
    fn cc_breed_pressed(&mut self, c: &mut Create) {
        let id = c.hover_breed;
        if id == 0 || id == c.breed {
            return;
        }
        c.breed = id;
        c.head = 0;
        self.cc_main_respec(c);
        c.scene_dirty = true;
        // main character joins the scene (EnableVisibility), prefs saved, label and text refreshed
        self.cc_save_prefs(c);
        self.cc_breed_label(c);
        self.cc_background_text(c);
        let sound = ["SM_Sandy_CC_Solitus_Female", "SM_Sandy_CC_Solitus_Male", "SM_Sandy_CC_Opifex_Female", "SM_Sandy_CC_Opifex_Male", "SM_Sandy_CC_Nano_Female", "SM_Sandy_CC_Nano_Male", "SM_Sandy_CC_Atrox"][id as usize - 1];
        self.cc_sound(sound);
        if let Some(b) = c.btns.iter_mut().find(|b| b.kind == B::Next) {
            b.enabled = true;
        }
    }

    /// `StopScene(code)`: the scene hides now, the module acts on `code` after 0.5 s.
    fn cc_stop(&mut self, c: &mut Create, code: i32) {
        c.shown = false;
        c.btns.clear();
        c.hover = None;
        c.pressed = None;
        for w in [c.info_w.take(), c.name_w.take()].into_iter().flatten() {
            self.gui.close_window(w);
        }
        c.stop = Some((0.5, code));
    }

    fn cc_next(&mut self, c: &mut Create) {
        self.cc_sound("SM_Sandy_CC_GUI_Select");
        match c.cur {
            Some(Sc::Breed) => {
                self.cc_save_prefs(c);
                c.rig.start(&[1, 1]);
            }
            Some(Sc::Appearance) => {
                self.cc_snap(c, &[2, 3]);
                let mut keys = vec![vec![2, 3], vec![3, 1]];
                if c.prof > 0 {
                    keys.push(vec![3, 1, c.prof as u32]);
                }
                c.rig.set_transition(&[2, 1], 1.9, keys);
                c.rig.start(&[2, 1]);
                self.cc_save_prefs(c);
            }
            Some(Sc::Profession) => {
                // Shadowlands professions (Keeper 5, Shade 6) are refused without the expansion
                let sl = self.char_list.expansions & 2 != 0 || self.char_list.sl_profs_enabled != 0;
                if !sl && (c.prof == 5 || c.prof == 6) {
                    let t = self.text.by_key(1003, "CharCreation_SLOnlyProfession").unwrap_or_default();
                    self.message_box(&t);
                    self.cc_sound("SM_Sandy_CC_GUI_Error");
                    return;
                }
                let mut keys = vec![];
                if c.prof > 0 {
                    keys.push(vec![3, 1, c.prof as u32]);
                }
                keys.extend([vec![3, 1], vec![3, 3, 1], vec![3, 3, 2], vec![4, 1]]);
                c.rig.set_transition(&[4, 1], 3.4, keys);
                c.rig.start(&[4, 1]);
            }
            _ => {}
        }
        self.cc_stop(c, 0x65);
    }

    fn cc_prev(&mut self, c: &mut Create) {
        self.cc_sound("SM_Sandy_CC_GUI_Select");
        match c.cur {
            Some(Sc::Appearance) => {
                self.cc_snap(c, &[2, 3]);
                c.rig.set_transition(&[2, 1, 3], 1.9, vec![vec![2, 3], vec![1, 1]]);
                c.rig.start(&[2, 1, 3]);
                self.cc_save_prefs(c);
            }
            Some(Sc::Profession) => {
                let mut keys = vec![];
                keys.push(if c.prof > 0 { vec![3, 1, c.prof as u32] } else { vec![3, 1] });
                if c.prof > 0 {
                    keys.push(vec![3, 1]);
                }
                keys.push(vec![2, 1]);
                c.rig.set_transition(&[2, 2], 1.9, keys);
                c.rig.start(&[2, 2]);
            }
            Some(Sc::Name) => {
                if let Some(w) = c.name_w {
                    let _ = self.gui.text(w, "name"); // `CCSelectedName` pref (never read back)
                }
                let mut keys = vec![vec![4, 1], vec![3, 3, 2], vec![3, 3, 1], vec![3, 1]];
                if c.prof > 0 {
                    keys.push(vec![3, 1, c.prof as u32]);
                }
                c.rig.set_transition(&[4, 2], 3.7, keys);
                c.rig.start(&[4, 2]);
            }
            _ => {}
        }
        self.cc_stop(c, 0x66);
    }

    // ---- exit --------------------------------------------------------------------------------------------------

    /// `CharCreateModule_t::AskExitMessage`: "ExitCC" yes / no box; the scene is deactivated meanwhile.
    fn cc_ask_exit(&mut self, c: &mut Create) {
        let Some(w) = self.open_centered("CharacterActivateWindow") else { return };
        let q = self.text.by_key(1003, "CharCreation_ExitQuestion").unwrap_or_default();
        self.gui.set_text(w, "confirmation_text", &q);
        self.gui.set_text(w, "cancel_btn", "No");
        self.gui.resize_window(w, WindowSize::Preferred);
        self.recenter(w);
        self.dialog_w = Some((w, DialogKind::ExitCc));
        c.shown = false; // Deactivate (the widgets stay, hidden)
        for w in [c.info_w, c.name_w].into_iter().flatten() {
            self.gui.set_window_visible(w, false);
        }
    }

    /// `ExitMessageResult`: Yes quits, No reactivates the scene.
    pub(in crate::play) fn cc_exit_answer(&mut self, yes: bool, host: &mut Host) {
        self.close_dialog();
        let Some(c) = self.cc.as_mut() else { return };
        if yes {
            c.st = St::Gone;
            self.cc_quit(host);
        } else {
            c.shown = true;
            for w in [c.info_w, c.name_w].into_iter().flatten() {
                self.gui.set_window_visible(w, true);
            }
        }
    }

    /// `QuitCharacterCreation`: prefs saved, back to the character selection.
    pub(super) fn cc_quit(&mut self, host: &mut Host) {
        self.cc_close_windows();
        self.prefs.save();
        self.back_to_selection(host);
    }

    // ---- name scene / network ----------------------------------------------------------------------------------

    /// Text changes of the name input: `SlotTextInput` refuses non letters / digits; `UpdateButtonState`.
    pub(in crate::play) fn create_text_changed(&mut self, window: WindowId, text: &str) -> bool {
        let Some(mut c) = self.cc.take() else { return false };
        let mine = c.name_w == Some(window);
        if mine {
            let clean: String = text.chars().filter(|&ch| name_char_ok(ch)).collect();
            if clean != text {
                self.gui.set_text(window, "name", &clean);
            }
            self.cc_update_finish(&mut c);
        }
        self.cc = Some(c);
        mine
    }

    /// `NameScene_t::SetState(code)`: error text of the server (LHConnect / CharacterCreate feedback).
    fn cc_name_error(&mut self, c: &mut Create, code: i32) {
        c.pending_suggest = false;
        if let Some(key) = name_state_key(code) {
            let t = self.cc_text(key);
            self.cc_message(c, &t, if code == 0x1e || code == 0x1f { None } else { Some("SM_Sandy_CC_GUI_Error") });
        } else {
            eprintln!("character creation: unknown state {code:#x}");
        }
        c.name_locked = false;
        self.cc_update_finish(c);
    }

    /// `NameScene_t::Message(text)`: the text goes to global signal 0x184 (receiver UNRESOLVED), here a notice box; the
    /// name scene falls back to state 0 and the error sound plays. The original also asks for `SM_Sandy_CC_Name` /
    /// `SM_Sandy_CC_Nick`, but no sound bank defines them (`GetSoundPointer` → 0, `PlaySample(0)` returns; docs/screens.md
    /// §12), so those callers pass `None` — silence is the faithful result.
    fn cc_message(&mut self, c: &mut Create, text: &str, sound: Option<&str>) {
        self.message_box(text);
        for w in [c.info_w, c.name_w].into_iter().flatten() {
            self.gui.set_window_visible(w, false);
        }
        c.shown = false;
        if let Some(sound) = sound {
            self.cc_sound(sound);
        }
        c.message_open = true;
        // `SetState(0)` → `UpdateButtonState`: Finish follows the name field again (Back stays disabled, as in the original)
        c.name_locked = false;
        self.cc_update_finish(c);
    }

    /// `NameScene_t::SetState(0x1009)` with the suggest flag: `Client_t::SuggestNickName(breed, sex, profession)`.
    fn cc_suggest(&mut self, c: &mut Create) {
        let (b, s) = cc_breed_to_gc(c.breed).unwrap_or((1, 3));
        if self.fake {
            self.fake_events.push_back(LoginEvent::RandomName("Zalokon".into()));
        } else if let Some(sess) = &self.session {
            sess.request_random_name(b, s, cc_prof_to_gc(c.prof));
        }
    }

    /// `NameScene_t::StartServerCreation` (GUI 0x1011fcac): validate, then `SetState(0x1006)` → `Client_t::CreateCharacter`.
    fn cc_start_creation(&mut self, c: &mut Create) {
        let raw = c.name_w.map(|w| self.gui.text(w, "name")).unwrap_or_default();
        if raw.is_empty() {
            let t = self.cc_text("MustEnterName");
            return self.cc_message(c, &t, None);
        }
        let name = normalize_name(&raw);
        if let Err(e) = check_name(&name) {
            let t = self.cc_text(e.key());
            return self.cc_message(c, &t, None);
        }
        c.name_locked = true;
        let req = cc_request(c, name);
        eprintln!("create character: {req:?}");
        if self.fake {
            // `--fake-charlist`: an in-process fake login server — "Taken" is in use, anything else is created and handed off
            let taken = req.name.eq_ignore_ascii_case("taken");
            eprintln!("(--fake-charlist) fake server answers: {}", if taken { "NameInUse" } else { "CharacterCreated + ZoneHandoff" });
            if taken {
                self.fake_events.push_back(LoginEvent::CharacterCreateFailed { code: 0x1e });
            } else {
                self.fake_events.push_back(LoginEvent::CharacterCreated { character_id: 1 });
                self.fake_events.push_back(LoginEvent::ZoneHandoff { zone_ip: std::net::Ipv4Addr::LOCALHOST, zone_port: 0, character_id: 1 });
            }
            return;
        }
        if let Some(sess) = &self.session {
            sess.create_character(req);
        }
    }

    /// Session events while the creation module runs; true = handled.
    pub(in crate::play) fn create_session_event(&mut self, ev: &LoginEvent, host: &mut Host) -> bool {
        let Some(mut c) = self.cc.take() else { return false };
        let handled = match ev {
            LoginEvent::RandomName(n) => {
                if let Some(w) = c.name_w {
                    let clean: String = n.chars().filter(|&ch| name_char_ok(ch)).collect();
                    self.gui.set_text(w, "name", &clean); // SetSuggestedName
                }
                c.pending_suggest = false;
                self.cc_update_finish(&mut c);
                true
            }
            LoginEvent::CharacterCreateFailed { code } => {
                self.cc_name_error(&mut c, *code);
                true
            }
            LoginEvent::LoginError { code, .. } => {
                self.cc_name_error(&mut c, *code as i32);
                true
            }
            // 0x21 RequestRejected is not a name-scene state: Client+0x68 -> SlotLoginReply -> ShowError (docs/protocol.md), handled by the flow
            LoginEvent::CharacterCreated { .. } => {
                // SetState(0x1007): WasCharacterCreated = 1
                self.prefs.cc_created = true;
                self.prefs.save();
                true
            }
            LoginEvent::ZoneHandoff { .. } => {
                // `LoginOKMessage` → SetState(0x1008): StopScene(next) → the exit cinematic; `SetLoadingScreen(rand() % 3)`
                self.rng ^= self.rng << 13;
                self.rng ^= self.rng >> 17;
                self.rng ^= self.rng << 5;
                self.welcome_image = !self.rng.is_multiple_of(3);
                c.handoff = true;
                if c.cur.is_some() {
                    self.cc_stop(&mut c, 0x65);
                    c.st = St::Active;
                }
                true
            }
            _ => false,
        };
        let _ = host;
        self.cc = Some(c);
        handled
    }

    /// GUI events of the creation windows (message box answers).
    pub(in crate::play) fn create_message_closed(&mut self) {
        if let Some(c) = self.cc.as_mut() {
            if c.message_open {
                c.message_open = false;
                c.shown = true;
                for w in [c.info_w, c.name_w].into_iter().flatten() {
                    self.gui.set_window_visible(w, true);
                }
            }
        }
    }
}

/// `NameScene_t::SetState(0x1006)`: the `CreateCharacterRequest` of the module's selections.
fn cc_request(c: &Create, name: String) -> CreateCharacterRequest {
    let (breed, gender) = cc_breed_to_gc(c.breed).unwrap_or((1, 3));
    let head = c.heads.get(c.head).map(|h| h.mesh as i32); // `GetHeadMeshID(CCSelectedHead)`: the rdb 1010001 id
    CreateCharacterRequest { breed, gender, profession: cc_prof_to_gc(c.prof), head: head.unwrap_or(0), height: height_percent(c.height), width: c.size, name, starter_area: 0 }
}

#[cfg(test)]
impl Play {
    /// Live harness: the creation module with the given CC breed / profession selections (the scenes' clicks are not driven) sends the
    /// request `NameScene_t::StartServerCreation` would. The module must be initialised (`cc_try_init`), the selections are set like the picks.
    pub(in crate::play) fn live_create(&mut self, name: &str, breed: i32, prof: i32) -> bool {
        let Some(mut c) = self.cc.take() else { return false };
        if c.actors.is_empty() {
            self.cc = Some(c);
            return false;
        }
        c.breed = breed;
        c.prof = prof;
        c.head = 0;
        let (b, s, _) = CC_BREEDS[(breed - 1).clamp(0, 6) as usize];
        if let Some((breed, gender)) = super::gc_breed(b, s) {
            c.heads = super::head_table(&self.dir, breed, gender);
        }
        let req = cc_request(&c, name.to_string());
        eprintln!("create character: {req:?}");
        if let Some(sess) = &self.session {
            sess.create_character(req);
        }
        self.cc = Some(c);
        true
    }
}
