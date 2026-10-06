//! `~/Library/Application Support/aomac/prefs.txt`: `key=value` lines. Stands in for the original's `prefs/Prefs.xml`
//! `LauncherConfig` archive (docs/screens.md §3.3): remembered account names (never passwords), the selected account,
//! plus the `SelectedCharacter` row and the chosen server.

use std::path::PathBuf;

/// `IndependentPrefs` `CCSelectedBreed/Height/Size/Head/Profession` of the character creation (docs/screens.md §12).
#[derive(Clone, Default)]
pub struct CcPrefs {
    pub breed: i32,
    pub height: i32,
    pub size: i32,
    pub head: i32,
    pub profession: i32,
}

#[derive(Default)]
pub struct Prefs {
    pub cc: CcPrefs,
    /// `WasCharacterCreated`: the next creation starts from scratch.
    pub cc_created: bool,
    pub accounts: Vec<String>,
    pub selected_account: usize,
    pub selected_character: i32,
    pub server: String,
}

/// aomac's prefs directory (stand-in for the client's `prefs/`; also holds `CharacterViewer.xml`).
pub fn dir() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(d) = TEST_DIR.with(|t| t.borrow().clone()) {
        return Some(d);
    }
    if let Some(d) = std::env::var_os("AOMAC_PREFS_DIR") {
        return Some(d.into()); // tests / scratch runs
    }
    Some(PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support/aomac"))
}

#[cfg(test)]
thread_local! { static TEST_DIR: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) }; }

/// Test scratch directory, isolated to the current test thread without mutating the process environment.
#[cfg(test)]
pub fn set_test_dir(d: impl Into<PathBuf>) {
    let d = d.into();
    TEST_DIR.with(|t| *t.borrow_mut() = Some(d));
}

fn path() -> Option<PathBuf> {
    Some(dir()?.join("prefs.txt"))
}

impl Prefs {
    pub fn load() -> Self {
        let mut p = Prefs { selected_character: -1, cc: CcPrefs { height: 1, size: 1, ..Default::default() }, ..Default::default() };
        for line in path().and_then(|f| std::fs::read_to_string(f).ok()).unwrap_or_default().lines() {
            match line.split_once('=') {
                Some(("account", v)) if !v.is_empty() => p.accounts.push(v.into()),
                Some(("selected_account", v)) => p.selected_account = v.parse().unwrap_or(0),
                Some(("selected_character", v)) => p.selected_character = v.parse().unwrap_or(-1),
                Some(("server", v)) => p.server = v.into(),
                Some(("cc_breed", v)) => p.cc.breed = v.parse().unwrap_or(0),
                Some(("cc_height", v)) => p.cc.height = v.parse().unwrap_or(1),
                Some(("cc_size", v)) => p.cc.size = v.parse().unwrap_or(1),
                Some(("cc_head", v)) => p.cc.head = v.parse().unwrap_or(0),
                Some(("cc_profession", v)) => p.cc.profession = v.parse().unwrap_or(0),
                Some(("cc_created", v)) => p.cc_created = v == "1",
                _ => {}
            }
        }
        p
    }

    /// `SaveAccounts` equivalent: the selected account is the one in `current` (the name just logged in).
    pub fn remember(&mut self, current: &str) {
        if !self.accounts.iter().any(|a| a == current) {
            self.accounts.push(current.into());
        }
        self.selected_account = self.accounts.iter().position(|a| a == current).unwrap_or(0);
        self.save();
    }

    pub fn remove(&mut self, name: &str) {
        self.accounts.retain(|a| a != name);
        self.selected_account = 0;
        self.save();
    }

    pub fn save(&self) {
        let Some(f) = path() else { return };
        let mut text = String::new();
        for a in &self.accounts {
            text += &format!("account={}\n", a.replace('\n', ""));
        }
        text += &format!("selected_account={}\nselected_character={}\nserver={}\n", self.selected_account, self.selected_character, self.server.replace('\n', ""));
        let c = &self.cc;
        text += &format!("cc_breed={}\ncc_height={}\ncc_size={}\ncc_head={}\ncc_profession={}\ncc_created={}\n", c.breed, c.height, c.size, c.head, c.profession, u8::from(self.cc_created));
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&f, text) {
            eprintln!("prefs: {}: {e}", f.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallel_test_directories_leave_native_environment_unchanged() {
        let before = std::env::var_os("AOMAC_PREFS_DIR");
        let start = std::sync::Barrier::new(4);
        std::thread::scope(|scope| {
            for i in 0..4 {
                let before = &before;
                let start = &start;
                scope.spawn(move || {
                    let scratch = std::env::temp_dir().join(format!("aomac-prefs-isolation-{i}"));
                    start.wait();
                    for _ in 0..100 {
                        set_test_dir(&scratch);
                        assert_eq!(dir(), Some(scratch.clone()));
                        assert_eq!(std::env::var_os("AOMAC_PREFS_DIR"), *before);
                    }
                });
            }
        });
    }
}

