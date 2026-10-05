//! The character-selection preview, as `CharacterViewer_c`/`CCCharacter_t` do it (docs/screens.md §5.5): the naked default
//! character of the entry's breed/sex with its first head, `idle-stand` played once, then after every finished clip
//! `rand() % 115 < 23` picks one of the 23 socials, else idle again.
//!
//! Poses are CPU-skinned on a worker thread (`load_player` per frame sample); the UI plays the cached frames. Socials are
//! built on demand: a clip that is not cached yet is requested and idle plays meanwhile (the original has no such delay).

use ao_formats::character::{self, Player, Role};
use ao_formats::screens::{self, CharSelectLook};
use ao_rdb::RecordStore;
use ao_scene::Scene;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};

/// Pose samples per second of the cached clips.
pub const FPS: f32 = 20.0;

pub enum Out {
    /// The textured bind/idle pose to upload.
    First(Box<Scene>),
    /// All frames of a clip (`None` = idle).
    Clip(Option<usize>, Vec<Scene>),
    Failed(String),
}

pub struct Worker {
    req: Sender<Option<usize>>,
    pub rx: Receiver<Out>,
}

impl Worker {
    /// Starts building the preview of `(breed, sex)`; dropping the worker stops the thread.
    pub fn start(dir: PathBuf, breed: i32, sex: i32) -> Worker {
        let (req, req_rx) = channel::<Option<usize>>();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            if let Err(e) = run(&dir, breed, sex, &req_rx, &tx) {
                let _ = tx.send(Out::Failed(format!("{e:#}")));
            }
        });
        Worker { req, rx }
    }

    /// Asks for the frames of social `i` (index into `screens::SOCIALS`) or idle (`None`).
    pub fn request(&self, clip: Option<usize>) {
        let _ = self.req.send(clip);
    }
}

fn frames(store: &RecordStore, look: &CharSelectLook, role: Role) -> anyhow::Result<Vec<Scene>> {
    let player = Player::new(look.breed, look.gender, look.skin, Some(look.head.0));
    let model = character::player_model(store, look.breed, look.gender)?;
    let anim = character::load_anim(store, character::role_anim(store, model, &role)?)?;
    let n = ((anim.duration / 1000.0 * FPS).round() as usize).max(1);
    (0..n)
        .map(|k| {
            let mut s = character::load_player(store, &player, Some((role.clone(), k as f32 / FPS)))?;
            s.textures.clear();
            Ok(s)
        })
        .collect()
}

fn run(dir: &std::path::Path, breed: i32, sex: i32, req: &Receiver<Option<usize>>, tx: &Sender<Out>) -> anyhow::Result<()> {
    let store = RecordStore::open(dir)?;
    let look = screens::char_select_look(&store, breed, sex)?;
    let player = Player::new(look.breed, look.gender, look.skin, Some(look.head.0));
    if tx.send(Out::First(Box::new(character::load_player(&store, &player, Some((Role::Idle, 0.0)))?))).is_err() {
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
        let role = clip.map_or(Role::Idle, |i| Role::Emote(screens::SOCIALS[i].to_string()));
        let t0 = std::time::Instant::now();
        let f = frames(&store, &look, role)?;
        if std::env::var_os("AOMAC_PERF").is_some() {
            eprintln!("preview clip {clip:?}: {} frames in {:.0} ms", f.len(), t0.elapsed().as_secs_f32() * 1000.0);
        }
        if tx.send(Out::Clip(clip, f)).is_err() {
            return Ok(());
        }
    }
}
