//! `~/Library/Application Support/aomac/prefs.txt`: `key=value` lines. Stands in for the original's `prefs/Prefs.xml`
//! `LauncherConfig` archive (docs/screens.md §3.3): remembered account names (never passwords), the selected account,
//! plus the `SelectedCharacter` row and the chosen server.

use std::path::PathBuf;

#[derive(Default)]
pub struct Prefs {
    pub accounts: Vec<String>,
    pub selected_account: usize,
    pub selected_character: i32,
    pub server: String,
}

fn path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support/aomac/prefs.txt"))
}

impl Prefs {
    pub fn load() -> Self {
        let mut p = Prefs { selected_character: -1, ..Default::default() };
        for line in path().and_then(|f| std::fs::read_to_string(f).ok()).unwrap_or_default().lines() {
            match line.split_once('=') {
                Some(("account", v)) if !v.is_empty() => p.accounts.push(v.into()),
                Some(("selected_account", v)) => p.selected_account = v.parse().unwrap_or(0),
                Some(("selected_character", v)) => p.selected_character = v.parse().unwrap_or(-1),
                Some(("server", v)) => p.server = v.into(),
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
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&f, text) {
            eprintln!("prefs: {}: {e}", f.display());
        }
    }
}
