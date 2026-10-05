//! Click-to-focus / Tab order on the real LoginWindow (skipped without a client install).
use ao_gui::{Gui, InputEvent, Key, Modifiers, MouseButton, WindowSize};

fn click(g: &mut Gui, x: f32, y: f32) {
    g.input(InputEvent::MouseMove { x, y });
    g.input(InputEvent::MouseDown { x, y, button: MouseButton::Left });
    g.input(InputEvent::MouseUp { x, y, button: MouseButton::Left });
}

#[test]
fn click_and_tab_move_focus() {
    let Ok(mut g) = Gui::new(&ao_gui::client_dir(), None) else { return };
    let w = g.open_framed_window("LoginWindow", (40, 40), WindowSize::Preferred).unwrap();
    let c = |g: &Gui, n: &str| {
        let r = g.view_rect(w, n).unwrap();
        ((r.l + r.r) / 2.0, (r.t + r.b) / 2.0)
    };
    let (ux, uy) = c(&g, "username");
    click(&mut g, ux, uy);
    g.input(InputEvent::Text("abc".into()));
    let (px, py) = c(&g, "password");
    click(&mut g, px, py);
    g.input(InputEvent::Text("x".into()));
    assert_eq!(g.text(w, "password"), "x");
    assert_eq!(g.text(w, "username"), "abc");
    click(&mut g, ux, uy);
    g.input(InputEvent::Key { key: Key::Tab, pressed: true, mods: Modifiers::default() });
    g.input(InputEvent::Text("y".into()));
    assert_eq!(g.text(w, "password"), "y");
    assert_eq!(g.text(w, "username"), "abc");
}
