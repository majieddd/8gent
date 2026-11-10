//! Where 8gent keeps its files. One folder holds everything, so a test (or a second install) can point
//! `EIGHTGENT_HOME` somewhere else and never touch the real one.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Paths {
    pub home: PathBuf,
}

impl Paths {
    /// `EIGHTGENT_HOME`, else the platform's per-user data folder.
    pub fn from_env() -> Self {
        if let Some(dir) = std::env::var_os("EIGHTGENT_HOME").filter(|v| !v.is_empty()) {
            return Self { home: PathBuf::from(dir) };
        }
        Self { home: default_home() }
    }

    pub fn at(home: impl Into<PathBuf>) -> Self {
        Self { home: home.into() }
    }

    pub fn config(&self) -> PathBuf {
        self.home.join("config.toml")
    }
    /// Models the user added by hand; scanned models are found again at every start.
    pub fn models(&self) -> PathBuf {
        self.home.join("models.toml")
    }
    /// Loader definitions that add to, or replace, the built-in ones.
    pub fn loaders(&self) -> PathBuf {
        self.home.join("loaders")
    }
    pub fn logs(&self) -> PathBuf {
        self.home.join("logs")
    }
    pub fn state(&self) -> PathBuf {
        self.home.join("state")
    }
    pub fn engine_log(&self, loader: &str) -> PathBuf {
        self.logs().join(format!("engine-{loader}.log"))
    }
    /// The engine this service started, so a restarted service can find one it left behind.
    pub fn engine_record(&self) -> PathBuf {
        self.state().join("engine.json")
    }
    pub fn perf_log(&self) -> PathBuf {
        self.state().join("requests.jsonl")
    }

    pub fn ensure(&self) -> std::io::Result<()> {
        for dir in [self.home.clone(), self.loaders(), self.logs(), self.state()] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}

pub fn user_home() -> PathBuf {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn default_home() -> PathBuf {
    let home = user_home();
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/8gent")
    } else if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or(home.join("AppData/Roaming")).join("8gent")
    } else {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or(home.join(".local/share")).join("8gent")
    }
}

/// `~/x` and `{home}`, `{data}` in a path from a loader definition or the config.
pub fn expand(text: &str, paths: &Paths) -> String {
    let home = user_home();
    let mut out = text.replace("{data}", &paths.home.to_string_lossy()).replace("{home}", &home.to_string_lossy());
    if let Some(rest) = out.strip_prefix("~/") {
        out = home.join(rest).to_string_lossy().into_owned();
    }
    out
}

/// The first existing file among `candidates`, then `command` looked up on PATH.
pub fn find_program(candidates: &[String], command: Option<&str>, paths: &Paths) -> Option<PathBuf> {
    for c in candidates {
        let p = PathBuf::from(expand(c, paths));
        if p.is_file() {
            return Some(p);
        }
    }
    let command = command?;
    if command.contains(std::path::MAIN_SEPARATOR) {
        let p = PathBuf::from(expand(command, paths));
        return p.is_file().then_some(p);
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for name in program_names(command) {
            let p = dir.join(&name);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

fn program_names(command: &str) -> Vec<String> {
    if cfg!(windows) && Path::new(command).extension().is_none() {
        vec![format!("{command}.exe"), format!("{command}.cmd"), command.to_string()]
    } else {
        vec![command.to_string()]
    }
}
