//! Login-flow screen data against the real client; skips cleanly when it is not installed.

use ao_formats::character::Role;
use ao_formats::screens::*;
use ao_rdb::RecordStore;
use std::path::PathBuf;

fn client() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("HOME")?).join("Games/ProjectRubiKa/client");
    dir.join("cd_image/rdb.db").exists().then_some(dir)
}

#[test]
fn char_create_camera_file() {
    let Some(dir) = client() else { return };
    let c = CharCreateCameras::load(&dir).unwrap();
    assert_eq!((c.cameras.len(), c.transitions.len()), (53, 23));
    let first = c.camera(&[1]).unwrap();
    assert_eq!((first.pos, first.fov_deg), ([27.4753, 2.0, 7.52619], 60.0));
    assert_eq!(c.camera(&[5, 2]).unwrap().pos[0], -1536.83);
    let t = c.transition(&[1, 1, 1]).unwrap();
    assert_eq!((t.duration_s, t.keyframes.len(), t.keyframes[12].clone()), (25.0, 13, vec![1, 1]));
    // every keyframe names an existing camera
    for t in &c.transitions {
        for k in &t.keyframes {
            assert!(c.camera(k).is_some(), "transition {:?} refers to missing camera {k:?}", t.id);
        }
    }
}

#[test]
fn login_world() {
    let Some(dir) = client() else { return };
    let store = RecordStore::open(&dir).unwrap();
    let ids: Vec<_> = login_world_ids(&store).unwrap().into_iter().map(|x| x.1).collect();
    assert_eq!(ids, [200350, 200967, 200997, 201000, 201002]);
    let s = login_world_scene(&store, 1).unwrap();
    assert_eq!(s.instances.len(), 5);
    assert_eq!(s.instances[0].transform[3][1], -0.01);
    assert_eq!(s.spawn, Some([0.657694, 2.27458, 10.5489]));
}

#[test]
fn char_select_preview() {
    let Some(dir) = client() else { return };
    let store = RecordStore::open(&dir).unwrap();
    let f = char_select_look(&store, 1, 3).unwrap();
    assert_eq!((f.model, f.idle_clip.1), (5927, 10135), "solitus_female.cir / female_idle-stand_01_01.ani");
    assert_eq!(f.social_clips.len(), 23);
    assert!((f.position[1] - (2.27458 - 1.9 - 0.02)).abs() < 1e-4 && (f.position[2] + 15.5489).abs() < 1e-4, "{:?}", f.position);
    let a = char_select_look(&store, 4, 3).unwrap(); // atrox is forced to male
    assert_eq!((a.model, a.idle_clip.1, a.position[1]), (5900, 9992, 2.27458 - 1.9));
    assert!(a.social_clips[22].0.starts_with("athrox_social-wave"));
    let scene = f.scene(&store, Some((Role::Idle, 0.0))).unwrap();
    assert!(!scene.instances.is_empty());
}

#[test]
fn texts_and_locations() {
    let Some(dir) = client() else { return };
    let t = TextDb::load(&dir).unwrap();
    assert_eq!(t.label("#Login"), "Login");
    assert_eq!(t.label("#RemoveAccount"), "Remove");
    assert_eq!(t.label("#NewCharacter"), "New Character");
    assert_eq!(t.by_key(CAT_GUI, "AO_Loading").as_deref(), Some("..Anarchy Online is loading.."));
    assert_eq!(t.by_key(CAT_GUI, "AvailableCharSlots").as_deref(), Some("%d/%d slots available"));
    assert_eq!(t.by_id(2004, 1).as_deref(), Some("Soldier"));
    assert_eq!(t.by_id(1005, 601).as_deref(), Some("Solitus"));
    assert_eq!(t.by_key(506, "NotChosenYet").as_deref(), Some("Not chosen yet."));
    let names = pfnr_names(&dir).unwrap();
    assert_eq!(location_text(&names, 566), "Newland City (566)");
}
