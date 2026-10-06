use super::super::dvalue::Kind;
use super::*;
use ao_formats::screens::TextDb;
use ao_gui::DrawList;
use ao_render::{Frontend, Host, Offscreen};

fn dvalue_res(d: &DValues) -> impl Fn(&str, &str) -> Option<i64> + '_ {
    move |k, n| (k == "dvalue").then(|| d.get_i64(n)).flatten()
}

struct Rig {
    gui: Gui,
    opt: HudOptions,
    d: DValues,
}

impl Frontend for Rig {
    fn gui(&self) -> &Gui {
        &self.gui
    }
    fn input(&mut self, ev: InputEvent, _host: &mut Host) {
        self.send(ev);
    }
    fn frame(&mut self, dt: f32, _size: (u32, u32), _host: &mut Host) -> DrawList {
        self.tick();
        self.gui.frame(dt)
    }
}

impl Rig {
    fn new() -> Option<Rig> {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui/Default/OptionPanel/Root.xml").exists() {
            eprintln!("skipping: no client");
            return None;
        }
        let labels = TextDb::load(&dir).unwrap();
        let gui = Gui::new(&dir, Some(Box::new(move |s: &str| Some(labels.label(s)).filter(|r| r != s)))).unwrap();
        let mut rig = Rig { gui, opt: HudOptions::new(&dir, (1280, 800)).unwrap(), d: DValues::new(&dir) };
        rig.opt.open(&mut rig.gui, &rig.d);
        rig.tick();
        Some(rig)
    }

    fn tick(&mut self) {
        let d = &self.d;
        self.opt.update(&mut self.gui, d, &dvalue_res(d));
    }

    fn send(&mut self, ev: InputEvent) {
        self.opt.input(&mut self.gui, &mut self.d, &ev);
        for e in self.gui.input(ev) {
            self.opt.event(&mut self.gui, &e, &mut self.d);
        }
        self.tick();
    }

    fn win(&self) -> WindowId {
        self.opt.window().unwrap()
    }

    /// Selects the page showing the option `var` (a tree click) and returns the control's view name.
    fn show(&mut self, var: &str) -> String {
        let (page, name) = {
            let w = self.opt.win.as_ref().unwrap();
            let c = w.controls.iter().find(|c| c.opt.var == var).unwrap_or_else(|| panic!("no control for {var}"));
            (c.page, c.name.clone())
        };
        let leaf = self.opt.win.as_ref().unwrap().leaves[page].clone();
        let id = self.win();
        // a leaf inside a closed folder: open its folders first, as the user does
        for f in self.opt.win.as_ref().unwrap().folders.clone() {
            if leaf.starts_with(&f) {
                self.gui.list_open_folder(id, TREE, &f, true);
            }
        }
        self.gui.list_select(id, TREE, &leaf, true, true);
        self.send(InputEvent::MouseMove { x: 0.0, y: 0.0 });
        self.opt.win.as_mut().unwrap().selected = page;
        self.gui.select_child(id, PAGES, Some(page));
        self.send(InputEvent::MouseMove { x: 0.0, y: 0.0 });
        // scroll the page (its `ScrollView`) until the control is inside the viewport
        let pg = format!("pg{page}");
        for _ in 0..3 {
            let (Some(c), Some(v)) = (self.gui.view_rect(id, &name), self.gui.view_rect(id, &pg)) else { break };
            if c.t < v.t || c.b > v.b {
                let off = self.gui.scroll_offset(id, &pg);
                self.gui.set_scroll_offset(id, &pg, off + (c.t - v.t) - 20.0);
                self.send(InputEvent::MouseMove { x: 0.0, y: 0.0 });
            }
        }
        name
    }

    fn click(&mut self, view: &str) {
        let r = self.gui.view_rect(self.win(), view).unwrap_or_else(|| panic!("no view {view}"));
        let (x, y) = (r.l + 3.0, r.t + 3.0);
        self.send(InputEvent::MouseMove { x, y });
        self.send(InputEvent::MouseDown { x, y, button: MouseButton::Left });
        self.send(InputEvent::MouseUp { x, y, button: MouseButton::Left });
    }
}

#[test]
fn config_archive_round_trips_and_reads_the_shipped_template() {
    let c = Config { frame: [10.0, 20.0, 460.0, 500.0], folders: vec!["GUI".into(), "Video".into()], selected: "GUIControl Center".into(), tab: 1, scroll: 3.5 };
    assert_eq!(Config::parse(&c.archive()), Some(c.clone()));
    // the shape of `prefs/NewChar/Prefs.xml` (TinyXML single-quoted strings)
    let t = r#"<Archive name="OptionWindowConfig" code="0"><Rect name="WindowFrame" value="Rect(200.000000,200.000000,651.000000,670.000000)" /><Bool name="WindowPinButtonState" value="false" /><String name="open_panel_folders" value='&quot;GUI&quot;' /><String name="selected_panel" value='&quot;GUIControl Center&quot;' /><Int32 name="selected_tab" value="0" /></Archive>"#;
    let c = Config::parse(t).unwrap();
    assert_eq!((c.frame, c.folders.as_slice(), c.selected.as_str()), ([200.0, 200.0, 651.0, 670.0], &["GUI".to_string()][..], "GUIControl Center"));
}

#[test]
fn label_paths_split_into_folder_levels() {
    let l = levels("GUI/Control Center");
    assert_eq!(l, [("GUI".into(), "GUI".into(), false), ("GUIControl Center".into(), "Control Center".into(), true)]);
    assert_eq!(levels("Audio"), [("Audio".into(), "Audio".into(), true)]);
}

#[test]
fn every_control_is_bound_to_a_real_variable_with_the_client_default() {
    let dir = ao_gui::client_dir();
    let Ok(root) = std::fs::read_to_string(dir.join("cd_image/gui/Default/OptionPanel/Root.xml")) else { return eprintln!("skipping: no client") };
    let d = DValues::new(&dir);
    let pages = model::parse(&root);
    assert!(pages.len() >= 14, "{} pages", pages.len());
    let mut missing = vec![];
    let mut n = 0;
    for p in &pages {
        for o in p.opts() {
            n += 1;
            let known = match o.ty {
                model::Ty::Variant => d.exists(&o.var),
                model::Ty::Int => d.prefs.int_range(&o.var, o.kind).is_some(),
                model::Ty::Float => d.prefs.float_range(&o.var, o.kind).is_some(),
            };
            if !known {
                missing.push(format!("{} ({:?} {:?})", o.var, o.ty, o.kind));
            }
        }
    }
    assert!(n > 140, "{n} controls");
    // every variable is registered by the shipped defaults (Variables / MainPrefs / LoginPrefs / CharPrefs.xml, SetDefaultLoginPrefs / CharPrefs)
    assert!(missing.is_empty(), "options without a stored variable: {missing:?}");
    // spot-check defaults against the client files
    let v = |n: &str| d.get(n).cloned();
    assert_eq!(v("CCFadeLow"), Some(Variant::Float(0.33)));
    assert_eq!(v("CCFadeHigh"), Some(Variant::Float(0.85)));
    assert_eq!(v("CCFadeDelay"), Some(Variant::Float(2.0)));
    assert_eq!(v("ChatTextFadeDelay"), Some(Variant::Float(8.0)));
    assert_eq!(v("MasterVolume"), Some(Variant::Float(1.0)));
    assert_eq!(v("BattlemusicMode"), Some(Variant::Int(3)));
    assert_eq!(d.prefs.get_float("ViewDistance", Kind::Login), Some(0.8));
}

#[test]
fn window_opens_with_the_stored_values_and_clicks_write_them_back() {
    let Some(mut r) = Rig::new() else { return };
    // check box (variant DValue): `cc_compass` defaults to true
    let c = r.show("cc_compass");
    assert!(r.gui.checked(r.win(), &c));
    r.click(&c);
    assert!(!r.d.flag("cc_compass"), "the click wrote the DValue");
    assert!(!r.gui.checked(r.win(), &c));
    r.click(&c);
    assert!(r.d.flag("cc_compass"));
    // radio group (variant): `BattlemusicMode` defaults to 3 ("Full")
    let g = r.show("BattlemusicMode");
    assert_eq!(r.gui.radio_value(r.win(), &g), Some(3));
    r.click(&format!("{g}_1"));
    assert_eq!(r.d.get_i64("BattlemusicMode"), Some(1));
    assert_eq!(r.gui.radio_value(r.win(), &g), Some(1));
    // int pref in the character set
    let c = r.show("MouseLagFix");
    assert!(!r.gui.checked(r.win(), &c));
    r.click(&c);
    assert_eq!(r.d.prefs.get_int("MouseLagFix", Kind::Char), Some(1));
    // bit of an int: `VisualFlags` bit 2 (helmet) is set in the default 31
    let bit = r.opt.win.as_ref().unwrap().controls.iter().find(|c| c.opt.var == "VisualFlags" && c.opt.bit == Some(2)).unwrap().name.clone();
    r.show("VisualFlags");
    assert!(r.gui.checked(r.win(), &bit));
    r.click(&bit);
    assert_eq!(r.d.get_i64("VisualFlags"), Some(31 & !4));
    // a change made elsewhere (`/option`) is mirrored by the control
    r.d.set_i64("VisualFlags", 31);
    r.tick();
    assert!(r.gui.checked(r.win(), &bit));
}

#[test]
fn enable_expression_gates_the_dependent_controls() {
    let Some(mut r) = Rig::new() else { return };
    // `view_enable_expression="dvalue:ToggleAllEffects"` on the effect check boxes
    let c = r.show("Wildlife");
    assert!(r.gui.is_enabled(r.win(), &c));
    assert!(r.d.prefs.get_int("Wildlife", Kind::Login) == Some(1));
    r.d.set_i64("ToggleAllEffects", 0);
    r.tick();
    assert!(!r.gui.is_enabled(r.win(), &c));
    r.click(&c);
    assert_eq!(r.d.prefs.get_int("Wildlife", Kind::Login), Some(1), "a disabled control does not write");
    r.d.set_i64("ToggleAllEffects", 1);
    r.tick();
    assert!(r.gui.is_enabled(r.win(), &c));
    r.click(&c);
    assert_eq!(r.d.prefs.get_int("Wildlife", Kind::Login), Some(0));
    // `dvalue:cc_section1 && dvalue:cc_left_bar` gates the left wing
    let w = r.show("cc_left_wing");
    assert!(r.gui.is_enabled(r.win(), &w));
    r.d.set_i64("cc_left_bar", 0);
    r.tick();
    assert!(!r.gui.is_enabled(r.win(), &w));
}

#[test]
fn slider_drag_writes_the_value_and_shows_it_formatted() {
    let Some(mut r) = Rig::new() else { return };
    let s = r.show("CCFadeLow");
    let id = r.win();
    let rect = r.gui.view_rect(id, &s).unwrap();
    let width = r.gui.canvas_size(id, &s).0 as f32;
    assert!(width > 60.0, "the slider fills the page: {width}");
    let n = &s[1..];
    // default 0.33 shown as "33%" in `value_fmt`
    assert_eq!(r.gui.text(id, &format!("v{n}")), "<font color=#70C4D0>33%</font>");
    // grab the knob and drag to the far right: the value is clamped to the DValue's max (1.0)
    let left = rect.l + knob_x(0.33, 0.0, 1.0, width);
    r.send(InputEvent::MouseDown { x: left + 3.0, y: rect.t + 4.0, button: MouseButton::Left });
    r.send(InputEvent::MouseMove { x: rect.l + width * 2.0, y: rect.t + 4.0 });
    assert_eq!(r.d.get("CCFadeLow"), Some(&Variant::Float(1.0)));
    assert_eq!(r.gui.text(id, &format!("v{n}")), "<font color=#70C4D0>100%</font>");
    // half way
    r.send(InputEvent::MouseMove { x: rect.l + 3.0 + (width - KNOB_W) / 2.0, y: rect.t + 4.0 });
    let Some(Variant::Float(v)) = r.d.get("CCFadeLow").cloned() else { panic!() };
    assert!((v - 0.5).abs() < 0.02, "{v}");
    r.send(InputEvent::MouseUp { x: 0.0, y: 0.0, button: MouseButton::Left });
    let held = r.d.get("CCFadeLow").cloned();
    r.send(InputEvent::MouseMove { x: rect.l + 10.0, y: rect.t + 4.0 });
    assert_eq!(r.d.get("CCFadeLow").cloned(), held, "released: moving no longer drags");
    // a float pref slider (`ViewDistance`, 0..1, shown x100 as %)
    let vd = r.show("ViewDistance");
    assert_eq!(r.gui.text(id, &format!("v{}", &vd[1..])), "<font color=#70C4D0>80%</font>");
    // an out of reach press (not on the knob) grabs nothing
    let rc = r.gui.view_rect(id, &vd).unwrap();
    r.send(InputEvent::MouseDown { x: rc.r - 2.0, y: rc.t + 4.0, button: MouseButton::Left });
    r.send(InputEvent::MouseMove { x: rc.l, y: rc.t + 4.0 });
    r.send(InputEvent::MouseUp { x: 0.0, y: 0.0, button: MouseButton::Left });
    assert_eq!(r.d.view_distance(), 0.8);
}

#[test]
fn settings_survive_a_restart() {
    let Some(mut r) = Rig::new() else { return };
    let tmp = std::env::temp_dir().join("aomac-options-persist");
    let _ = std::fs::remove_dir_all(&tmp);
    r.d.open_user(&tmp, "acct", 7);
    let c = r.show("cc_compass");
    r.click(&c);
    let c = r.show("MouseLagFix");
    r.click(&c);
    let s = r.show("CCFadeHigh");
    let rect = r.gui.view_rect(r.win(), &s).unwrap();
    let width = r.gui.canvas_size(r.win(), &s).0 as f32;
    let left = rect.l + knob_x(0.85, 0.0, 1.0, width);
    r.send(InputEvent::MouseDown { x: left + 3.0, y: rect.t + 4.0, button: MouseButton::Left });
    r.send(InputEvent::MouseMove { x: rect.l + 3.0 + (width - KNOB_W) / 4.0, y: rect.t + 4.0 });
    r.send(InputEvent::MouseUp { x: 0.0, y: 0.0, button: MouseButton::Left });
    assert!(!r.d.take_changed().is_empty());
    // closing stores the window (`OptionWindowConfig`); the flow saves on every change
    r.opt.close(&mut r.gui, &mut r.d);
    r.d.save_user();
    let dir = ao_gui::client_dir();
    let mut again = DValues::new(&dir);
    again.open_user(&tmp, "acct", 7);
    assert!(!again.flag("cc_compass"));
    assert_eq!(again.prefs.get_int("MouseLagFix", Kind::Char), Some(1));
    let Some(Variant::Float(h)) = again.get("CCFadeHigh").cloned() else { panic!() };
    assert!((h - 0.25).abs() < 0.05, "{h}");
    let Some(Variant::Archive(a)) = again.get(CONFIG) else { panic!("OptionWindowConfig not saved") };
    assert!(a.contains("WindowFrame") && a.contains("selected_panel"), "{a}");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn quit_buttons_ask_for_quit_and_camp() {
    let Some(mut r) = Rig::new() else { return };
    r.click("b1");
    r.click("b2");
    assert_eq!(r.opt.take_actions(), [Action::Quit, Action::Camp]);
}

#[test]
fn closing_and_reopening_restores_selection_and_frame() {
    let Some(mut r) = Rig::new() else { return };
    let var = "ChatTextFadeDelay";
    r.show(var);
    let page = r.opt.win.as_ref().unwrap().selected;
    let id = r.win();
    r.gui.set_window_outer_frame(id, (50, 60, 500, 420));
    r.opt.close(&mut r.gui, &mut r.d);
    assert!(r.opt.window().is_none());
    r.opt.open(&mut r.gui, &r.d);
    r.tick();
    assert_eq!(r.opt.win.as_ref().unwrap().selected, page);
    assert_eq!(r.gui.window_outer_frame(r.win()), Some((50, 60, 500, 420)));
}

/// `AOMAC_SHOT_DIR=/tmp/x cargo test --release -p aomac -- options_shots`: the Preferences window on its first page, the control-centre page, and two others.
#[test]
fn options_shots() {
    let Some(mut r) = Rig::new() else { return };
    let size = (900, 740);
    let mut off = Offscreen::new(&r, size).unwrap();
    r.opt.set_screen(size);
    let mut snap = |r: &mut Rig, name: &str| {
        let mut list = DrawList::default();
        for _ in 0..3 {
            list = off.frame(r, 0.016);
        }
        if let Some(dir) = std::env::var_os("AOMAC_SHOT_DIR") {
            std::fs::create_dir_all(&dir).unwrap();
            off.png(r, &list, &std::path::Path::new(&dir).join(format!("{name}.png"))).unwrap();
        }
    };
    snap(&mut r, "options_first");
    for (var, name) in [("cc_compass", "options_cc"), ("Wildlife", "options_effects"), ("BattlemusicMode", "options_audio"), ("ChatTextShadowOffset", "options_chat"), ("MouseWheel", "options_mouse")] {
        r.show(var);
        snap(&mut r, name);
    }
}

#[test]
fn control_prefs_follow_the_dvalues() {
    use super::super::controls::ControlPrefs;
    let mut d = DValues::default();
    d.prefs = super::super::dvalue::IndepPrefs::with_defaults();
    assert_eq!(ControlPrefs::from_dvalues(&d), ControlPrefs::default());
    d.load_config(
        r#"<Root><Value name="MouseTurnSensitivity" value="5" min="1" max="20"/><Value name="ZoomSpeed" value="30"/><Value name="LMBMouseLook" value="false"/><Value name="MouseWheel" value="2"/><Value name="ShowMyCharacter" value="true"/></Root>"#,
        super::super::dvalue::CAT_CHAR,
        true,
    );
    d.prefs.set_int("MouseLookInverted", 1, Kind::Login);
    let p = ControlPrefs::from_dvalues(&d);
    assert_eq!((p.mouse_turn_sensitivity, p.zoom_speed, p.lmb_mouse_look, p.mouse_wheel, p.show_my_character, p.mouse_look_inverted), (5.0, 30.0, false, 2, true, true));
}

#[test]
fn esc_closes_only_windows_whose_option_was_set_when_they_opened() {
    use super::super::hud::WindowKind as K;
    let dir = ao_gui::client_dir();
    if !dir.join("cd_image/gui/Default/OptionPanel/Root.xml").exists() {
        return eprintln!("skipping: no client");
    }
    let mut d = DValues::new(&dir);
    let mut o = HudOptions::new(&dir, (800, 600)).unwrap();
    // defaults of LoginPrefs.xml: esc_wear / esc_perkwindow / esc_optionpanel true, esc_inventory / esc_nano false
    for k in [K::Character, K::Inventory, K::Perks, K::Nano, K::Skills] {
        o.arm_esc(k, &d);
    }
    d.set_i64("esc_inventory", 1); // changed while open: no effect
    assert_eq!(o.take_esc(), [K::Character, K::Perks]);
    o.arm_esc(K::Inventory, &d);
    o.arm_esc(K::Options, &d);
    o.disarm_esc(K::Options);
    assert_eq!(o.take_esc(), [K::Inventory]);
}
