//! Renders the original character-selection backdrop: `cargo run --release -p aomac --example login_shot -- <breed 1-4> <sex 2|3> out.png [stage]`.
//! Camera, backdrop and preview placement are the RE'd ones of `docs/screens.md` §2 / §5.5 (`ao_formats::screens`).
use ao_formats::character::Role;
use ao_formats::screens::*;
use ao_rdb::RecordStore;
use anyhow::Result;
use std::path::PathBuf;

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let (breed, sex): (i32, i32) = (a.get(1).map_or(1, |s| s.parse().unwrap()), a.get(2).map_or(3, |s| s.parse().unwrap()));
    let out = PathBuf::from(a.get(3).map_or("login_shot.png", |s| s));
    let stage: u32 = a.get(4).map_or(1, |s| s.parse().unwrap());
    let dir = PathBuf::from(std::env::var("HOME")?).join("Games/ProjectRubiKa/client");
    let store = RecordStore::open(&dir)?;
    let mut scene = login_world_scene(&store, stage)?;
    let look = char_select_look(&store, breed, sex)?;
    let ch = look.scene(&store, Some((Role::Idle, 0.0)))?;
    let (mo, p) = (scene.meshes.len(), ao_to_render(look.position));
    scene.textures.extend(ch.textures);
    scene.meshes.extend(ch.meshes);
    for mut i in ch.instances {
        i.mesh += mo;
        for (t, d) in i.transform[3].iter_mut().zip(p) {
            *t += d;
        }
        scene.instances.push(i);
    }
    let (eye, at) = (scene.spawn.unwrap(), scene.spawn_look_at.unwrap());
    ao_render::render_to_png(&scene, eye, at, 1024, 768, &out)
}
