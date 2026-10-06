//! `fade_group` views follow the real pointer (`FadeGroupController_c`, docs/gui.md §10.8): dim at start, 0.2 s to `CCFadeHigh` on hover,
//! `CCFadeDelay` + 1 s back to `CCFadeLow`, parameter changes apply live. Skipped without a client install (the engine needs its skin).
use ao_gui::{Gui, InputEvent, WindowSize};

const XML: &str = r#"<root><View view_layout="vertical">
  <View view_layout="vertical" fade_group="g1"><TextView name="a" value="Group one"/></View>
  <TextView name="b" value="Group two" fade_group="g2"/>
  <TextView name="c" value="No group"/>
</View></root>"#;

fn run(g: &mut Gui, secs: f32) {
    for _ in 0..(secs / 0.01).round() as u32 {
        g.frame(0.01);
    }
}

fn over(g: &mut Gui, w: usize, name: &str) {
    let r = g.view_rect(w, name).unwrap();
    g.input(InputEvent::MouseMove { x: r.l + 2.0, y: r.t + 2.0 });
}

#[test]
fn groups_follow_the_pointer() {
    let Ok(mut g) = Gui::new(&ao_gui::client_dir(), None) else { return eprintln!("skipping: no client") };
    let w = g.open_window_xml("t", XML, (10, 10), WindowSize::Fixed(300, 200)).unwrap();
    g.set_screen_size(800, 600);
    run(&mut g, 0.05);
    assert_eq!((g.fade_alpha("g1"), g.fade_alpha("g2")), (Some(0.33), Some(0.33)));
    assert_eq!(g.view_alpha(w, "b"), Some(0.33));
    assert_eq!(g.view_alpha(w, "c"), Some(1.0), "views without a group are untouched");

    // the hovered view is a child of the group view: the walk up the parents finds the group
    over(&mut g, w, "a");
    run(&mut g, 0.1);
    let mid = g.fade_alpha("g1").unwrap();
    assert!(mid > 0.5 && mid < 0.7, "{mid}");
    run(&mut g, 0.11);
    assert_eq!((g.fade_alpha("g1"), g.fade_alpha("g2")), (Some(0.85), Some(0.33)));

    // onto a view without a group: g1 waits CCFadeDelay, then falls over 1 s
    over(&mut g, w, "c");
    run(&mut g, 1.9);
    assert_eq!(g.fade_alpha("g1"), Some(0.85));
    run(&mut g, 1.2);
    assert_eq!(g.fade_alpha("g1"), Some(0.33));

    // live: a new CCFadeDelay is used the next time the pointer leaves; new Low / High apply at once
    g.set_fade_params(0.33, 0.85, 4.0);
    over(&mut g, w, "b");
    run(&mut g, 0.3);
    assert_eq!(g.fade_alpha("g2"), Some(0.85));
    over(&mut g, w, "c");
    run(&mut g, 3.9);
    assert_eq!(g.fade_alpha("g2"), Some(0.85));
    run(&mut g, 1.2);
    assert_eq!(g.fade_alpha("g2"), Some(0.33));
    g.set_fade_params(0.5, 0.85, 4.0);
    assert_eq!((g.fade_alpha("g1"), g.view_alpha(w, "b")), (Some(0.5), Some(0.33)), "alpha reaches the views on the next frame");
    run(&mut g, 0.02);
    assert_eq!(g.view_alpha(w, "b"), Some(0.5));
}
