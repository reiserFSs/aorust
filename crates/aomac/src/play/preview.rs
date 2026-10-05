//! Character-select 3D preview: maps a `CharacterInfo` to a creation-screen player, builds a looping idle
//! animation as pre-posed frames on a background thread and lays out the stage (backdrop quad + lights).

use ao_formats::character::{self, Breed, Gender, Player, Role, Skin};
use ao_net::msg::CharacterInfo;
use ao_rdb::RecordStore;
use ao_scene::{Environment, Instance, Mesh, Scene, Submesh, Texture, TextureKey, Vertex};
use glam::{Mat4, Vec3};
use std::path::Path;
use std::sync::mpsc::Sender;

/// Pose sampling rate of the idle loop (CPU re-posing; frames are cached, not computed per display frame).
pub const FPS: f32 = 12.0;
const MAX_FRAMES: usize = 72;
const BACKDROP: TextureKey = TextureKey { rdb_type: 0xAAAA, id: 1 };

pub enum Msg {
    /// First pose with textures; later poses carry none.
    First(Box<Scene>),
    Frame(Box<Scene>),
    Failed(String),
}

/// `CharacterInfo` breed/gender/head -> player. Values are CellAO's enums (Breed 1..4 = Solitus, Opifex, Nanomage,
/// Atrox; Gender 1 male / 2 female) -- `[inference]`, protocol.md §6 notes the same. Unknown values fall back to
/// Solitus male. Skin tone is not in the character list (solitus: caucasian). `head` is matched against the head
/// number `NN` first, then used as 0-based index into the available heads (guess; PRK head ids unverified).
pub fn player(store: &RecordStore, info: &CharacterInfo) -> Player {
    let breed = match info.breed {
        2 => Breed::Opifex,
        3 => Breed::Nanomage,
        4 => Breed::Atrox,
        _ => Breed::Solitus,
    };
    let gender = if info.gender == 2 { Gender::Female } else { Gender::Male };
    let heads = character::player_heads(store, breed, gender, Skin::Caucasian).unwrap_or_default();
    let head = heads.iter().find(|h| h.0 as i32 == info.head).or(heads.get(info.head.max(0) as usize)).map(|h| h.0);
    Player { breed, gender, skin: Skin::Caucasian, head }
}

/// Posed idle frames for `info`, streamed over `tx` (First, then Frame..., or Failed).
pub fn build(dir: &Path, info: CharacterInfo, tx: Sender<(u64, Msg)>, gen: u64) {
    let send = |m| tx.send((gen, m)).is_ok();
    let run = || -> anyhow::Result<()> {
        let t0 = std::time::Instant::now();
        let store = RecordStore::open(dir)?;
        let p = player(&store, &info);
        let model = character::player_model(&store, p.breed, p.gender)?;
        let clip = character::role_anim(&store, model, &Role::Idle).and_then(|a| character::load_anim(&store, a));
        let pose = |t: f32| character::load_player(&store, &p, clip.is_ok().then_some((Role::Idle, t)));
        send(Msg::First(Box::new(pose(0.0)?)));
        let n = clip.as_ref().map_or(1, |c| ((c.duration / 1000.0 * FPS).round() as usize).clamp(1, MAX_FRAMES));
        let period = n as f32 / FPS;
        for k in 1..n {
            let mut s = pose(k as f32 * period / n as f32)?;
            s.textures.clear();
            if !send(Msg::Frame(Box::new(s))) {
                break; // stale: the UI moved on to another character
            }
        }
        if std::env::var_os("AOMAC_PERF").is_some() {
            eprintln!("preview of {}: {n} idle frames in {:.0} ms", info.name, t0.elapsed().as_secs_f32() * 1000.0);
        }
        Ok(())
    };
    if let Err(e) = run() {
        send(Msg::Failed(format!("{e:#}")));
    }
}

/// Character-select stage state: cached idle poses of the character on screen.
pub struct Stage {
    frames: Vec<Scene>,
    centre: Vec3,
    quad: Option<usize>,
    shown: usize,
}

impl Stage {
    /// `first` is the textured pose. Returns the stage, the scene to upload (first pose + backdrop quad 10 m behind the
    /// character + dark environment) and the camera (eye, at).
    pub fn new(mut first: Scene, backdrop: Option<&Texture>) -> (Self, Scene, Vec3, Vec3) {
        let at0 = Vec3::from(first.spawn_look_at.unwrap_or([0.0, 1.0, 0.0]));
        let eye0 = Vec3::from(first.spawn.unwrap_or([0.0, 1.0, -3.0]));
        // Camera looks along +Z (screen right = -X); shift both so the character sits left of the list panel.
        let shift = -Vec3::X * (eye0 - at0).length() * 0.3;
        let mut pose0 = first.clone();
        pose0.textures.clear();
        let quad = backdrop.map(|t| {
            let (w, h) = (26.7, 20.0);
            let (z, cx, cy) = (at0.z + 10.0, at0.x + shift.x, at0.y);
            let v = |u: f32, vv: f32| Vertex { pos: [cx - (u - 0.5) * w, cy + (0.5 - vv) * h, z], normal: [0.0, 0.0, -1.0], uv: [u, vv], ..Default::default() };
            let mut sm = Submesh::new(vec![0, 1, 2, 0, 2, 3], Some(BACKDROP));
            sm.prelit = true;
            sm.two_sided = true;
            first.textures.insert(BACKDROP, t.clone());
            first.meshes.push(Mesh { vertices: vec![v(0.0, 0.0), v(0.0, 1.0), v(1.0, 1.0), v(1.0, 0.0)], submeshes: vec![sm] });
            first.instances.push(Instance { mesh: first.meshes.len() - 1, transform: ao_scene::IDENTITY });
            first.meshes.len() - 1
        });
        first.environment = Some(Environment {
            sky_color: [0.004, 0.004, 0.012],
            fog_color: [0.004, 0.004, 0.012],
            fog_start: 500.0,
            fog_end: 1000.0,
            ambient: [0.42, 0.42, 0.48],
            sun_color: [0.85, 0.8, 0.75],
            sun_dir: [-0.35, 0.45, -0.8],
        });
        (Self { frames: vec![pose0], centre: at0, quad, shown: 0 }, first, eye0 + shift, at0 + shift)
    }

    pub fn push(&mut self, pose: Scene) {
        self.frames.push(pose);
    }

    /// The pose at `t` seconds with the character turned `angle` rad about its vertical axis, as a
    /// `Host::repose` delta (vertices only when the pose changed).
    pub fn pose(&mut self, t: f32, angle: f32) -> Scene {
        let k = (t * FPS) as usize % self.frames.len();
        let f = &self.frames[k];
        let c = Vec3::new(self.centre.x, 0.0, self.centre.z);
        let spin = Mat4::from_translation(c) * Mat4::from_rotation_y(angle) * Mat4::from_translation(-c);
        let mut instances: Vec<Instance> =
            f.instances.iter().map(|i| Instance { mesh: i.mesh, transform: (spin * Mat4::from_cols_array_2d(&i.transform)).to_cols_array_2d() }).collect();
        if let Some(mesh) = self.quad {
            instances.push(Instance { mesh, transform: ao_scene::IDENTITY });
        }
        let meshes = if k == self.shown { vec![] } else { f.meshes.clone() };
        self.shown = k;
        Scene { meshes, instances, ..Default::default() }
    }
}
