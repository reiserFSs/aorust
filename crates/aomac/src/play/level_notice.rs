//! GUI NotifyButton (0x10052fa0), got_* callbacks 0x1002de8e/1002dfe7/1002e140.
use super::dvalue::{DValues, Kind};
use ao_gui::{Event, Gui, WindowId, WindowSize};

const NOTICES: [(&str, &str, &str, i32); 3] = [
    ("got_ip", "GFX_GUI_NEW_IP", "skill_window", 273),
    ("got_perk", "GFX_GUI_NEW_PERK", "perk_window", 333),
    ("got_tech", "GFX_GUI_NEW_TECH", "research_window", 393),
];

#[derive(Default)]
pub(super) struct LevelNotice {
    windows: [Option<WindowId>; 3],
}

impl LevelNotice {
    pub(super) fn update(&mut self, gui: &mut Gui, values: &mut DValues, screen: (u32, u32)) {
        let enabled = values.prefs.get_int("ShowIPPerkbutton", Kind::Login).unwrap_or(1) != 0;
        for (i, &(flag, gfx, kind, bottom)) in NOTICES.iter().enumerate() {
            if values.flag(flag) && (!enabled || values.flag(kind)) {
                values.set_i64(flag, 0);
            }
            if !values.flag(flag) {
                if let Some(window) = self.windows[i].take() { gui.close_window(window); }
                continue;
            }
            if self.windows[i].is_none() {
                let xml = format!("<root><Button name=\"notify\" gfxid_raised=\"{gfx}\" gfxid_pressed=\"{gfx}\" gfxid_hover=\"{gfx}\"/></root>");
                match gui.open_window_xml(flag, &xml, (screen.0 as i32 - 60, screen.1 as i32 - bottom), WindowSize::Preferred) {
                    Ok(window) => self.windows[i] = Some(window),
                    Err(error) => eprintln!("hud: {flag} notification: {error:#}"),
                }
            }
        }
    }

    pub(super) fn event(&self, event: &Event) -> Option<&'static str> {
        let Event::Clicked { window, .. } = event else { return None };
        self.windows.iter().position(|id| *id == Some(*window)).map(|i| NOTICES[i].2)
    }

    pub(super) fn opened(values: &mut DValues, kind: &str) {
        if let Some(&(flag, _, _, _)) = NOTICES.iter().find(|notice| notice.2 == kind) {
            values.set_i64(flag, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_notice_assets_geometry_click_and_preference() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui/Default/Graphics.uvgi").exists() { return; }
        let mut gui = Gui::new(&dir, None).unwrap();
        let mut values = DValues::new(&dir);
        let mut notices = LevelNotice::default();
        for &(flag, _, _, _) in &NOTICES { values.set_i64(flag, 1); }
        notices.update(&mut gui, &mut values, (1280, 800));
        for (i, &(_, art, target, bottom)) in NOTICES.iter().enumerate() {
            let gfx = gui.gfx_id(art).unwrap();
            assert_eq!(gfx, 0xaf + i as u32);
            let window = notices.windows[i].unwrap();
            let size = gui.gfx().size(ao_gui::GfxId(gfx));
            assert!(size.0 > 0 && size.1 > 0);
            assert_eq!(gui.window_size(window), size);
            assert_eq!(gui.outer_size(window), size, "no title or border chrome");
            assert_eq!(gui.window_pos(window), (1220, 800 - bottom));
            assert_eq!(notices.event(&Event::Clicked { window, view: "notify".into(), item: None }), Some(target));
        }
        values.prefs.set_int("ShowIPPerkbutton", 0, Kind::Login);
        notices.update(&mut gui, &mut values, (1280, 800));
        assert!(notices.windows.iter().all(Option::is_none));
        assert!(NOTICES.iter().all(|notice| !values.flag(notice.0)));
    }

    #[test]
    fn notice_art_uses_native_button_palette_and_hover_overlay() {
        use ao_gui::{DrawCmd, InputEvent, MouseButton};
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui/Default/Graphics.uvgi").exists() { return; }
        let mut gui = Gui::new(&dir, None).unwrap();
        gui.set_screen_size(1280, 800);
        let mut values = DValues::new(&dir);
        values.set_i64("got_ip", 1);
        let mut notices = LevelNotice::default();
        notices.update(&mut gui, &mut values, (1280, 800));
        let art = gui.gfx_id("GFX_GUI_NEW_IP").unwrap();
        let layers = |gui: &mut Gui| gui.frame(0.0).cmds.into_iter().filter_map(|cmd| {
            if let DrawCmd::Gfx { id, tint, alpha, .. } = cmd {
                if id.0 == art { return Some((tint, alpha)); }
            }
            None
        }).collect::<Vec<_>>();
        assert_eq!(layers(&mut gui), vec![([0x80, 0xe9, 0xf3], 0.85)]);
        let r = gui.view_rect(notices.windows[0].unwrap(), "notify").unwrap();
        let (x, y) = (r.l + 5.0, r.t + 5.0);
        gui.input(InputEvent::MouseMove { x, y });
        assert_eq!(layers(&mut gui), vec![([0x80, 0xe9, 0xf3], 0.85), ([0xa5, 0xff, 0xdb], 0.85)]);
        gui.input(InputEvent::MouseDown { x, y, button: MouseButton::Left });
        assert_eq!(layers(&mut gui), vec![([0xff, 0xff, 0xcc], 0.85), ([0xa5, 0xff, 0xdb], 0.85)]);
    }

    #[test]
    fn opening_notice_target_clears_only_its_flag() {
        let mut values = DValues::new(std::path::Path::new("/nonexistent-aomac-client"));
        for &(flag, _, _, _) in &NOTICES { values.set_i64(flag, 1); }
        LevelNotice::opened(&mut values, "skill_window");
        assert!(!values.flag("got_ip"));
        assert!(values.flag("got_perk") && values.flag("got_tech"));
    }
}
