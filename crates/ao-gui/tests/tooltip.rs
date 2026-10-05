//! Tooltips on a view built from XML (`tooltip=` / `tooltip_body=`): 500 ms rest timer, placement by `Window::MoveToMouse`,
//! closing on mouse buttons (skipped without a client install).
use ao_gui::{Gui, InputEvent, MouseButton, WindowSize};

const XML: &str = r#"<root><View view_layout="vertical">
  <TextView name="a" value="Hover me" tooltip="Title only"/>
  <TextView name="b" value="Second line" tooltip="Skill" tooltip_body="This is a longer description that has to be wrapped at some width"/>
  <TextView name="c" value="No tip"/>
</View></root>"#;

fn mv(g: &mut Gui, x: f32, y: f32) {
    g.input(InputEvent::MouseMove { x, y });
}

#[test]
fn tooltip_after_rest_and_close() {
    let Ok(mut g) = Gui::new(&ao_gui::client_dir(), None) else { return };
    let w = g.open_window_xml("t", XML, (10, 10), WindowSize::Fixed(400, 300)).unwrap();
    g.set_screen_size(800, 600);
    let ra = g.view_rect(w, "a").unwrap();
    let (x, y) = (ra.l + 2.0, ra.t + 2.0);
    mv(&mut g, x, y);
    g.frame(0.4);
    assert!(g.tooltip_shown().is_none(), "shown before 500 ms");
    g.frame(0.2);
    assert_eq!(g.tooltip_shown().unwrap().0, "Title only");
    // mouse in the left/upper half: window at mouse + (16, 32)
    let r = g.tooltip_rect().unwrap();
    assert_eq!((r.l, r.t), (x.floor() + 16.0, y.floor() + 32.0));
    // moving over a view without tip closes it
    let rc = g.view_rect(w, "c").unwrap();
    mv(&mut g, rc.l + 1.0, rc.t + 1.0);
    assert!(g.tooltip_shown().is_none());
    g.frame(1.0);
    assert!(g.tooltip_shown().is_none());
    // body tooltip, then a click closes it
    let rb = g.view_rect(w, "b").unwrap();
    mv(&mut g, rb.l + 1.0, rb.t + 1.0);
    g.frame(0.6);
    let (t, b) = g.tooltip_shown().unwrap();
    assert_eq!(t, "Skill");
    assert!(b.starts_with("This is"));
    let list = g.frame(0.0);
    assert!(list.cmds.len() > 20);
    g.input(InputEvent::MouseDown { x: rb.l, y: rb.t, button: MouseButton::Left });
    assert!(g.tooltip_shown().is_none());
}

#[test]
fn tooltip_flips_in_lower_right() {
    let Ok(mut g) = Gui::new(&ao_gui::client_dir(), None) else { return };
    g.set_screen_size(800, 600);
    g.input(InputEvent::MouseMove { x: 700.0, y: 500.0 });
    g.show_tooltip("Flip", "");
    let r = g.tooltip_rect().unwrap();
    // `x = mouse.x - (w + 1)`, `y = mouse.y - (h + 1)`: the window ends just left of / above the pointer
    assert_eq!((r.r + 1.0, r.b + 1.0), (700.0, 500.0));
}
