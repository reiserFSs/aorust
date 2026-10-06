//! Border buttons (close / pin / `?`) and the hover fade of unpinned windows (`WndBorder::CreateBorderIcons` 0x1015aba4, `SetHelpFile` 0x1015ae58,
//! `WindowController_c::FadeWindows` 0x1015803c; docs/gui.md §6.2). Skipped without a client install.
use ao_gui::{Event, Gui, InputEvent, MouseButton, WindowSize};

const XML: &str = r#"<root><View view_layout="vertical"><TextView name="t" value="x"/></View></root>"#;

fn gui() -> Option<Gui> {
    Gui::new(&ao_gui::client_dir(), None).ok()
}

fn click(g: &mut Gui, x: f32, y: f32) -> Vec<Event> {
    g.input(InputEvent::MouseMove { x, y });
    let mut v = g.input(InputEvent::MouseDown { x, y, button: MouseButton::Left });
    v.extend(g.input(InputEvent::MouseUp { x, y, button: MouseButton::Left }));
    v
}

fn open(g: &mut Gui, pos: (i32, i32)) -> usize {
    g.open_tabbed_window_xml("t", "T", XML, pos, WindowSize::Fixed(300, 150)).unwrap()
}

#[test]
fn pin_help_and_close_buttons_act_on_release() {
    let Some(mut g) = gui() else { return };
    let w = open(&mut g, (100, 100));
    // outer (100,100)..(409,280): close box x 387..401, pin 369..383, `?` 351..365 (y 105..119)
    assert_eq!(g.window_outer_frame(w), Some((100, 100, 310, 181)));
    assert!(click(&mut g, 375.0, 112.0).contains(&Event::FramePin { window: w, pinned: true }));
    assert!(g.window_pinned(w));
    assert!(click(&mut g, 375.0, 112.0).contains(&Event::FramePin { window: w, pinned: false }));
    assert!(!g.window_pinned(w));
    // no `?` before SetHelpFile
    assert!(click(&mut g, 357.0, 112.0).is_empty());
    g.set_window_help(w, Some("The Skill Window.html"));
    assert!(click(&mut g, 357.0, 112.0).contains(&Event::FrameHelp { window: w, url: "file://The Skill Window.html".into() }));
    assert!(click(&mut g, 394.0, 112.0).contains(&Event::CloseRequested { window: w }));
    // released outside the button: nothing
    g.input(InputEvent::MouseMove { x: 394.0, y: 112.0 });
    g.input(InputEvent::MouseDown { x: 394.0, y: 112.0, button: MouseButton::Left });
    g.input(InputEvent::MouseMove { x: 394.0, y: 140.0 });
    assert!(g.input(InputEvent::MouseUp { x: 394.0, y: 140.0, button: MouseButton::Left }).is_empty());
    // flag 0x800: no pin button
    g.set_window_pin_button(w, false);
    assert!(!click(&mut g, 375.0, 112.0).iter().any(|e| matches!(e, Event::FramePin { .. })));
}

#[test]
fn unpinned_windows_fade_when_the_pointer_leaves_them() {
    let Some(mut g) = gui() else { return };
    let (a, b) = (open(&mut g, (100, 100)), open(&mut g, (600, 100)));
    let c = open(&mut g, (100, 400));
    g.set_window_fade(c, false);
    g.input(InputEvent::MouseMove { x: 200.0, y: 200.0 }); // enter a
    g.input(InputEvent::MouseMove { x: 700.0, y: 200.0 }); // leave a for b
    g.frame(0.5);
    assert!((g.window_fade(a) - 0.65).abs() < 1e-4, "{}", g.window_fade(a)); // 1 -> 0.3 over 1 s
    g.frame(1.0);
    assert!((g.window_fade(a) - 0.3).abs() < 1e-4);
    assert_eq!(g.window_fade(b), 1.0);
    // back onto a: 0.2 s to full alpha; b (left) starts fading
    g.input(InputEvent::MouseMove { x: 200.0, y: 200.0 });
    g.frame(0.2);
    assert!((g.window_fade(a) - 1.0).abs() < 1e-4);
    g.frame(1.0);
    assert!((g.window_fade(b) - 0.3).abs() < 1e-4);
    // pinned and opted-out windows never fade
    g.set_window_pinned(a, true);
    g.input(InputEvent::MouseMove { x: 700.0, y: 200.0 });
    g.frame(2.0);
    assert_eq!(g.window_fade(a), 1.0);
    g.input(InputEvent::MouseMove { x: 200.0, y: 450.0 });
    g.input(InputEvent::MouseMove { x: 700.0, y: 200.0 });
    g.frame(2.0);
    assert_eq!(g.window_fade(c), 1.0);
}
