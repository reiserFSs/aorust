//! `CCCharacter_t` (GUI 0x1011ad5f): a naked player character of the creation world. Poses are CPU-skinned on a worker
//! thread (like `preview.rs`, but for any head / build); the UI thread plays the cached 20 Hz frames. After every finished
//! clip `rand() % 115 < 23` picks one of the 23 socials, else the idle clip (`CCCharacter_t::RunFunction`).

use ao_formats::character::{self, Breed, Gender, Player, Role, Skin};
use ao_formats::create::{ao_matrix_to_render, character_at_connector};
use ao_formats::screens::{self, SOCIALS};
use ao_rdb::RecordStore;
use ao_scene::Scene;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};

/// Pose samples per second of the cached clips.
pub const FPS: f32 = 20.0;

/// What defines the look of a character (`CCCharacter_t` +4 breed, +8 sex, +0xc head index, +0x10 build, +0x18 race).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spec {
    pub breed: Breed,
    pub gender: Gender,
    /// Head number `NN` of `head_<race><sex>NN.abiff` and its ethnicity.
    pub head: (u32, Skin),
    /// 0 thin, 1 normal, 2 fat.
    pub build: u8,
}

enum Out {
    First(Box<Scene>),
    Clip(Option<usize>, Vec<Scene>),
    Failed(String),
}

struct Worker {
    req: Sender<Option<usize>>,
    rx: Receiver<Out>,
}

impl Worker {
    fn start(dir: PathBuf, spec: Spec) -> Worker {
        let (req, req_rx) = channel::<Option<usize>>();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            if let Err(e) = run(&dir, spec, &req_rx, &tx) {
                let _ = tx.send(Out::Failed(format!("{e:#}")));
            }
        });
        Worker { req, rx }
    }
}

fn player(spec: Spec) -> Player {
    Player::new(spec.breed, spec.gender, spec.head.1, Some(spec.head.0))
}

fn frames(store: &RecordStore, spec: Spec, role: Role) -> anyhow::Result<Vec<Scene>> {
    let p = player(spec);
    let model = character::player_model_build(store, spec.breed, spec.gender, spec.build)?;
    let anim = character::load_anim(store, character::role_anim(store, model, &role)?)?;
    let n = ((anim.duration / 1000.0 * FPS).round() as usize).max(1);
    (0..n)
        .map(|k| {
            let mut s = character::load_player_build(store, &p, spec.build, Some((role.clone(), k as f32 / FPS)))?;
            s.textures.clear();
            Ok(s)
        })
        .collect()
}

fn run(dir: &std::path::Path, spec: Spec, req: &Receiver<Option<usize>>, tx: &Sender<Out>) -> anyhow::Result<()> {
    let store = RecordStore::open(dir)?;
    let first = character::load_player_build(&store, &player(spec), spec.build, Some((Role::Idle, 0.0)))?;
    if tx.send(Out::First(Box::new(first))).is_err() {
        return Ok(());
    }
    let mut done = vec![];
    let mut queue = vec![None];
    loop {
        let clip = match queue.pop() {
            Some(c) => c,
            None => match req.recv() {
                Ok(c) => c,
                Err(_) => return Ok(()),
            },
        };
        while let Ok(more) = req.try_recv() {
            queue.push(more);
        }
        if done.contains(&clip) {
            continue;
        }
        done.push(clip);
        let role = clip.map_or(Role::Idle, |i| Role::Emote(SOCIALS[i].to_string()));
        let f = frames(&store, spec, role)?;
        if tx.send(Out::Clip(clip, f)).is_err() {
            return Ok(());
        }
    }
}

/// Feet alignment of `CCCharacter_t::RunFunction` (Solitus / Nanomage only), added to the connector height:
/// female 0 → −0.02, 1 → −0.02, 2 → −0.07, thin +0.01 …; see `docs/screens.md` §12.
pub fn feet_offset(breed: Breed, gender: Gender, build: u8) -> f32 {
    if !matches!(breed, Breed::Solitus | Breed::Nanomage) {
        return 0.0;
    }
    match (gender, build) {
        (Gender::Female, 1) => -0.02,
        (Gender::Female, 2) => -0.07,
        (Gender::Female, _) => 0.01,
        (_, 2) => -0.05,
        _ => -0.02,
    }
}

/// Model scale of `Height_e` 0 / 1 / 2 (`RunFunction`: 0.95, 1.0, 1.05).
pub fn height_scale(h: i32) -> f32 {
    match h {
        0 => 0.95,
        2 => 1.05,
        _ => 1.0,
    }
}

pub struct Actor {
    dir: PathBuf,
    pub spec: Spec,
    worker: Worker,
    /// The bind-pose scene (meshes + textures) the GPU scene was built from; replaced when the spec changes.
    pub first: Option<Box<Scene>>,
    clips: HashMap<Option<usize>, Vec<Scene>>,
    playing: Option<(Option<usize>, f32)>,
    rng: u32,
    /// `PlayAnim(idle, loop = true)`: the idle clip repeats and no social is drawn (head close-up).
    pub loop_idle: bool,
    /// Connector placement in AO space (`MoveToConnector`).
    pub place: Option<[[f32; 4]; 4]>,
    pub height: i32,
    /// Set when a new bind pose arrived (the GPU scene must be rebuilt).
    pub fresh: bool,
    pub error: Option<String>,
    /// Frame currently shown (poses of the current clip).
    pub cur: Option<Scene>,
}

impl Actor {
    pub fn new(dir: PathBuf, spec: Spec, seed: u32) -> Actor {
        Actor {
            worker: Worker::start(dir.clone(), spec),
            dir,
            spec,
            first: None,
            clips: HashMap::new(),
            playing: None,
            rng: 0x9E37_79B9 ^ seed.wrapping_mul(0x85EB_CA6B),
            loop_idle: false,
            place: None,
            height: 1,
            fresh: false,
            error: None,
            cur: None,
        }
    }

    /// `SetBreed` / `SetHeadIndex` / build change: a new mesh set is built in the background; the old pose stays until it arrives.
    pub fn respec(&mut self, spec: Spec) {
        if spec != self.spec {
            self.spec = spec;
            self.worker = Worker::start(self.dir.clone(), spec);
            self.clips.clear();
            self.playing = None;
            self.error = None;
        }
    }

    /// `PlayAnim("idle-stand_01_01", loop)`.
    pub fn play_idle(&mut self, looped: bool) {
        self.loop_idle = looped;
        if self.clips.contains_key(&None) {
            self.playing = Some((None, 0.0));
        }
    }

    fn poll(&mut self) {
        while let Ok(m) = self.worker.rx.try_recv() {
            match m {
                Out::First(s) => {
                    self.first = Some(s);
                    self.cur = None;
                    self.fresh = true;
                }
                Out::Clip(c, f) => {
                    self.clips.insert(c, f);
                    if c.is_none() && self.playing.is_none() {
                        self.playing = Some((None, 0.0));
                    }
                }
                Out::Failed(e) => {
                    eprintln!("character creation actor: {e}");
                    self.error = Some(e);
                }
            }
        }
    }

    /// Advances the animation by `dt`; true when the frame shown changed.
    pub fn advance(&mut self, dt: f32) -> bool {
        self.poll();
        let Some((clip, t)) = self.playing else { return false };
        let len = self.clips[&clip].len();
        let (mut clip, mut t) = (clip, t + dt);
        if (t * FPS) as usize >= len {
            t = 0.0;
            if self.loop_idle {
                clip = None;
            } else {
                self.rng ^= self.rng << 13;
                self.rng ^= self.rng >> 17;
                self.rng ^= self.rng << 5;
                clip = None;
                if let Some(i) = screens::next_social(self.rng) {
                    if self.clips.contains_key(&Some(i)) {
                        clip = Some(i);
                    } else {
                        let _ = self.worker.req.send(Some(i)); // built in the background; idle plays meanwhile
                    }
                }
            }
        }
        self.playing = Some((clip, t));
        let frames = &self.clips[&clip];
        self.cur = Some(frames[((t * FPS) as usize).min(frames.len() - 1)].clone());
        true
    }

    /// The instance transform that places the character: `inst · scale · connector placement` (renderer space).
    pub fn transform(&self, inst: &[[f32; 4]; 4]) -> Option<[[f32; 4]; 4]> {
        let mut place = character_at_connector(&self.place?);
        place[3][1] += feet_offset(self.spec.breed, self.spec.gender, self.spec.build);
        let place = ao_matrix_to_render(&place);
        let s = height_scale(self.height);
        let mut scaled = *inst;
        for row in &mut scaled {
            for v in row.iter_mut().take(3) {
                *v *= s;
            }
        }
        scaled[3][3] = 1.0;
        Some(mul(&scaled, &place))
    }
}

fn mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut o = [[0.0; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            o[i][j] = (0..4).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    o
}

/// Möller–Trumbore against every triangle of the character's current pose (`VisualCATMesh_t::IsLineIntersecting`).
/// `origin`/`dir` in renderer space; returns the hit distance.
pub fn ray_hit(scene: &Scene, xf: &[[f32; 4]; 4], origin: glam::Vec3, dir: glam::Vec3) -> Option<f32> {
    let mut best: Option<f32> = None;
    for inst in &scene.instances {
        let Some(mesh) = scene.meshes.get(inst.mesh) else { continue };
        let m = glam::Mat4::from_cols_array_2d(&mul(&inst.transform, xf));
        let to = |i: u32| m.transform_point3(glam::Vec3::from(mesh.vertices[i as usize].pos));
        for sub in &mesh.submeshes {
            for t in sub.indices.as_chunks::<3>().0.iter() {
                let (a, b, c) = (to(t[0]), to(t[1]), to(t[2]));
                let (e1, e2) = (b - a, c - a);
                let p = dir.cross(e2);
                let det = e1.dot(p);
                if det.abs() < 1e-9 {
                    continue;
                }
                let inv = 1.0 / det;
                let s = origin - a;
                let u = s.dot(p) * inv;
                if !(0.0..=1.0).contains(&u) {
                    continue;
                }
                let q = s.cross(e1);
                let v = dir.dot(q) * inv;
                if v < 0.0 || u + v > 1.0 {
                    continue;
                }
                let d = e2.dot(q) * inv;
                if d > 0.0 && best.is_none_or(|b| d < b) {
                    best = Some(d);
                }
            }
        }
    }
    best
}
