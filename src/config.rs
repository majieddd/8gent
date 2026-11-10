//! `config.toml`: the endpoint's port, what is selected, and per-loader overrides.

use crate::paths::Paths;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 8-G-E-N on a phone keypad.
pub const DEFAULT_PORT: u16 = 8436;
/// Hermes refuses endpoints under 64,000 tokens, so a model that allows it starts here.
pub const DEFAULT_CONTEXT: u32 = 65_536;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryMode {
    /// Load the model when the service starts and keep it.
    Always,
    /// Load on the first request, unload after `idle_minutes` without one.
    Idle,
    /// Load on the first request and keep it.
    OnDemand,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub port: u16,
    /// Listen on every network interface, not only this computer.
    pub share: bool,
    /// Other ports the endpoint answers on as well: the addresses of the Home apps it replaced, so
    /// whatever was pointed at them keeps working.
    pub also_listen: Vec<u16>,
    /// When set, requests from other computers must carry it as a bearer token.
    pub api_key: Option<String>,
    /// The model name that always means "whatever is selected".
    pub served_name: String,
    pub memory_mode: MemoryMode,
    /// Minutes without a request before an idle model is unloaded (0: never).
    pub idle_minutes: f64,
    /// A request that names another model or loader switches to it.
    pub auto_switch: bool,
    /// Install newer engines by itself, once a day, when nothing has been asked for a while.
    pub auto_update: bool,
    pub selection: Selection,
    /// The context window last chosen for each model, used again whenever the model is selected
    /// without naming one.
    pub contexts: BTreeMap<String, u32>,
    pub loaders: BTreeMap<String, LoaderSettings>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            share: false,
            also_listen: vec![],
            api_key: None,
            served_name: "8gent".into(),
            memory_mode: MemoryMode::Idle,
            idle_minutes: 30.0,
            auto_switch: true,
            auto_update: true,
            selection: Selection::default(),
            contexts: BTreeMap::new(),
            loaders: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Selection {
    pub loader: String,
    pub model: String,
    /// 0: the default for the model (65,536, or the model's maximum when that is smaller).
    pub context: u32,
    pub thinking: bool,
}

impl Selection {
    pub fn is_set(&self) -> bool {
        !self.loader.is_empty() && !self.model.is_empty()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LoaderSettings {
    /// The engine's program, when it is not where the loader definition looks.
    pub program: Option<String>,
    /// An attached server's address, when it is not the default one.
    pub base_url: Option<String>,
    pub sessions: Option<u32>,
    pub extra_args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

impl Config {
    pub fn load(paths: &Paths) -> Result<Self, String> {
        match std::fs::read_to_string(paths.config()) {
            Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", paths.config().display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("{}: {e}", paths.config().display())),
        }
    }

    /// Written to a temporary file first, so a crash never leaves half a config.
    pub fn save(&self, paths: &Paths) -> Result<(), String> {
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = paths.config().with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, paths.config()).map_err(|e| format!("{}: {e}", paths.config().display()))
    }

    pub fn loader(&self, id: &str) -> LoaderSettings {
        self.loaders.get(id).cloned().unwrap_or_default()
    }
}
