//! `~/Library/Application Support/aomac/prefs.txt`: `key=value` lines. Never holds a password.

use std::path::PathBuf;

#[derive(Default)]
pub struct Prefs {
    pub remember: bool,
    pub username: String,
    pub server: String,
}

fn path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support/aomac/prefs.txt"))
}

impl Prefs {
    pub fn load() -> Self {
        let mut p = Prefs::default();
        for line in path().and_then(|f| std::fs::read_to_string(f).ok()).unwrap_or_default().lines() {
            match line.split_once('=') {
                Some(("remember", v)) => p.remember = v == "1",
                Some(("username", v)) => p.username = v.into(),
                Some(("server", v)) => p.server = v.into(),
                _ => {}
            }
        }
        p
    }

    /// Without `remember` the username is not written (an existing file is rewritten without it).
    pub fn save(&self) {
        let Some(f) = path() else { return };
        let user = if self.remember { self.username.as_str() } else { "" };
        let text = format!("remember={}\nusername={}\nserver={}\n", self.remember as u8, user.replace('\n', ""), self.server.replace('\n', ""));
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&f, text) {
            eprintln!("prefs: {}: {e}", f.display());
        }
    }
}
