//! A loader is described by a TOML file: how to find it, how to start it, what it can run and which
//! quirks of its API 8gent has to smooth over. Adding a loader means adding one of these files.

use crate::paths::{self, Paths};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The loader definitions that ship inside the binary. A file with the same id in `<home>/loaders`
/// replaces the built-in one.
const BUILTIN: &[(&str, &str)] = &[
    ("llamacpp", include_str!("../loaders/llamacpp.toml")),
    ("tensorfold", include_str!("../loaders/tensorfold.toml")),
    ("uzu", include_str!("../loaders/uzu.toml")),
    ("ollama", include_str!("../loaders/ollama.toml")),
    ("lmstudio", include_str!("../loaders/lmstudio.toml")),
    ("strata", include_str!("../loaders/strata.toml")),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// 8gent starts and stops the engine process itself.
    Managed,
    /// The engine is another app's server (Ollama, LM Studio, Strata); 8gent uses its address.
    Attach,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub homepage: String,
    pub kind: Kind,
    /// "macos-arm64", "windows-x64", "linux-x64"...; empty means every platform.
    #[serde(default)]
    pub platforms: Vec<String>,
    /// Model formats this loader runs: "gguf", "mlx", "uzu", "mirai"...
    #[serde(default)]
    pub formats: Vec<String>,
    #[serde(default)]
    pub detect: Detect,
    #[serde(default)]
    pub start: Option<Start>,
    #[serde(default)]
    pub attach: Option<Attach>,
    #[serde(default)]
    pub api: Api,
    #[serde(default)]
    pub capabilities: Capabilities,
    #[serde(default)]
    pub requires: Vec<Requirement>,
    /// Alternative engines of one loader (TensorFold's 0.6.6 Python line next to its native one).
    #[serde(default)]
    pub variants: BTreeMap<String, Variant>,
    #[serde(default)]
    pub update: Update,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Detect {
    pub command: Option<String>,
    pub paths: Vec<String>,
    /// Arguments that make the program print its version.
    pub version_args: Vec<String>,
    /// A file next to the program that holds the version (uzu Home's builds).
    pub version_file: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Start {
    pub args: Vec<String>,
    pub when: Vec<WhenArgs>,
    pub env: BTreeMap<String, String>,
    pub env_when: Vec<WhenEnv>,
    /// Answers 200 once the model is loaded.
    pub ready_path: String,
    pub ready_timeout_s: u64,
}

impl Default for Start {
    fn default() -> Self {
        Self {
            args: vec![],
            when: vec![],
            env: BTreeMap::new(),
            env_when: vec![],
            ready_path: "/v1/models".into(),
            ready_timeout_s: 600,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WhenArgs {
    #[serde(rename = "if")]
    pub condition: String,
    pub args: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WhenEnv {
    #[serde(rename = "if")]
    pub condition: String,
    pub set: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Variant {
    pub name: String,
    /// The program for this variant, first existing path wins; without one the loader's own program runs.
    pub program: Vec<String>,
    /// Arguments placed before the loader's start arguments (a script for an interpreter).
    pub args_prefix: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Attach {
    /// The server's OpenAI address, ending in /v1.
    pub base_url: String,
}

/// How this engine's HTTP API differs from the plain OpenAI one.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Api {
    /// The model name the engine expects in a request. "{model_id}" is 8gent's id for the model,
    /// "{ref}" the model's own reference; an attached server gets "{ref}" (its own model names).
    pub model_id: String,
    /// How a request switches thinking off when the client did not say:
    /// "none", "reasoning_effort" (sets it to `thinking_off`) or "enable_thinking" (sets the boolean).
    pub thinking: String,
    pub thinking_off: String,
    /// reasoning_effort names the engine accepts; others are mapped to the nearest. Empty: pass through.
    pub efforts: Vec<String>,
    /// Put a short user turn first when a history resumes with the assistant (uzu's template refuses it).
    pub role_normalize: bool,
    /// Lowercase fragments of the engine's own "prompt too long" message.
    pub overflow_markers: Vec<String>,
    /// The engine can report token counts at the end of a stream when asked.
    pub stream_usage: bool,
}

impl Default for Api {
    fn default() -> Self {
        Self {
            model_id: "{model_id}".into(),
            thinking: "none".into(),
            thinking_off: "none".into(),
            efforts: vec![],
            role_normalize: false,
            overflow_markers: ["context length", "context window", "context size", "context overflow", "prompt is too long"].map(String::from).to_vec(),
            stream_usage: true,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Capabilities {
    pub tools: bool,
    pub json_schema: bool,
    pub vision: bool,
    /// Serves the Anthropic Messages API (/v1/messages) itself.
    pub anthropic_api: bool,
    /// Conversations it can keep in memory at once (0: not configurable).
    pub sessions: u32,
    /// Sets the whole context window aside when the model loads (others grow it as the conversation does).
    pub reserves_context: bool,
}

/// "This loader, from this version on, needs at least this OS version."
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Requirement {
    pub os: String,
    pub min: String,
    #[serde(default)]
    pub since: Option<String>,
    #[serde(default)]
    pub note: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Update {
    /// "brew", "github", "app" (the engine's own app updates it) or "none".
    pub method: String,
    pub source: String,
}

impl Manifest {
    pub fn supports_platform(&self) -> bool {
        self.platforms.is_empty() || self.platforms.iter().any(|p| p == platform())
    }

    pub fn runs(&self, format: &str) -> bool {
        self.formats.iter().any(|f| f == format)
    }
}

/// "macos-arm64", "windows-x64", "linux-x64"...
pub fn platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "macos-arm64",
        ("macos", _) => "macos-x64",
        ("windows", "aarch64") => "windows-arm64",
        ("windows", _) => "windows-x64",
        ("linux", "aarch64") => "linux-arm64",
        ("linux", _) => "linux-x64",
        _ => "other",
    }
}

/// Built-in loaders, then the files in `<home>/loaders` (same id replaces, new id adds).
pub fn load_all(paths: &Paths) -> Result<Vec<Manifest>, String> {
    let mut by_id: BTreeMap<String, Manifest> = BTreeMap::new();
    let mut order: Vec<String> = vec![];
    for (id, text) in BUILTIN {
        let m: Manifest = toml::from_str(text).map_err(|e| format!("built-in loader {id}: {e}"))?;
        order.push(m.id.clone());
        by_id.insert(m.id.clone(), m);
    }
    if let Ok(entries) = std::fs::read_dir(paths.loaders()) {
        let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "toml")).collect();
        files.sort();
        for file in files {
            let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            let m: Manifest = toml::from_str(&text).map_err(|e| format!("{}: {e}", file.display()))?;
            if !by_id.contains_key(&m.id) {
                order.push(m.id.clone());
            }
            by_id.insert(m.id.clone(), m);
        }
    }
    Ok(order.into_iter().filter_map(|id| by_id.remove(&id)).collect())
}

// ------------------------------------------------------------------------------------------- templates

/// The values a start line or a condition can use.
#[derive(Clone, Debug, Default)]
pub struct Vars {
    pub model: String,
    pub model_id: String,
    pub port: u16,
    pub context: u32,
    pub sessions: u32,
    pub thinking: bool,
    pub os_version: String,
}

impl Vars {
    pub fn fill(&self, text: &str, paths: &Paths) -> String {
        let out = text
            .replace("{model}", &self.model)
            .replace("{ref}", &self.model)
            .replace("{model_id}", &self.model_id)
            .replace("{port}", &self.port.to_string())
            .replace("{context}", &self.context.to_string())
            .replace("{sessions}", &self.sessions.to_string());
        paths::expand(&out, paths)
    }

    /// `thinking`, `!thinking`, `macos<26.4`, `sessions>1`, `context>=65536`.
    pub fn holds(&self, condition: &str) -> bool {
        let c = condition.trim();
        if let Some(rest) = c.strip_prefix('!') {
            return !self.holds(rest);
        }
        for op in ["<=", ">=", "<", ">", "="] {
            if let Some((key, value)) = c.split_once(op) {
                let (key, value) = (key.trim(), value.trim());
                let left = match key {
                    "context" => self.context.to_string(),
                    "sessions" => self.sessions.to_string(),
                    k if k == std::env::consts::OS => self.os_version.clone(),
                    // a condition about another OS never holds here
                    "macos" | "windows" | "linux" => return false,
                    _ => return false,
                };
                let ord = compare_versions(&left, value);
                return match op {
                    "<=" => ord != std::cmp::Ordering::Greater,
                    ">=" => ord != std::cmp::Ordering::Less,
                    "<" => ord == std::cmp::Ordering::Less,
                    ">" => ord == std::cmp::Ordering::Greater,
                    _ => ord == std::cmp::Ordering::Equal,
                };
            }
        }
        match c {
            "thinking" => self.thinking,
            os if os == std::env::consts::OS => true,
            _ => false,
        }
    }
}

/// Numeric comparison of dotted versions ("26.4" > "26.2", "0.6.17" > "0.6.3"); non-digits end a part.
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let parts = |s: &str| -> Vec<u64> {
        s.trim_start_matches('v')
            .split('.')
            .map(|p| p.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or(0))
            .collect()
    };
    let (a, b) = (parts(a), parts(b));
    for i in 0..a.len().max(b.len()) {
        let ord = a.get(i).copied().unwrap_or(0).cmp(&b.get(i).copied().unwrap_or(0));
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
    }
    std::cmp::Ordering::Equal
}

/// The first dotted number in a program's version output ("version: 6240 (abc)" -> "6240").
pub fn first_version(text: &str) -> Option<String> {
    let mut best = String::new();
    let mut cur = String::new();
    for ch in text.chars().chain(std::iter::once(' ')) {
        if ch.is_ascii_digit() || (ch == '.' && !cur.is_empty()) {
            cur.push(ch);
        } else {
            let t = cur.trim_end_matches('.').to_string();
            if t.contains('.') && best.is_empty() {
                return Some(t);
            }
            if best.is_empty() && !t.is_empty() {
                best = t;
            }
            cur.clear();
        }
    }
    (!best.is_empty()).then_some(best)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_loaders_parse() {
        let paths = Paths::at(std::env::temp_dir().join("8gent-none"));
        let all = load_all(&paths).unwrap();
        let ids: Vec<&str> = all.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["llamacpp", "tensorfold", "uzu", "ollama", "lmstudio", "strata"]);
        for m in &all {
            match m.kind {
                Kind::Managed => assert!(m.start.is_some(), "{} needs [start]", m.id),
                Kind::Attach => assert!(m.attach.is_some(), "{} needs [attach]", m.id),
            }
        }
    }

    #[test]
    fn conditions() {
        let v = Vars { thinking: true, sessions: 2, context: 65536, os_version: "26.2".into(), ..Default::default() };
        assert!(v.holds("thinking") && !v.holds("!thinking"));
        assert!(v.holds("sessions>1") && !v.holds("sessions>2") && v.holds("context>=65536"));
        let os = std::env::consts::OS;
        assert!(v.holds(&format!("{os}<26.4")) && !v.holds(&format!("{os}>=26.4")));
        assert!(!v.holds("plan9<3"));
    }

    #[test]
    fn versions() {
        use std::cmp::Ordering::*;
        assert_eq!(compare_versions("0.6.17", "0.6.3"), Greater);
        assert_eq!(compare_versions("26.2", "26.4"), Less);
        assert_eq!(compare_versions("v1.2", "1.2.0"), Equal);
        assert_eq!(first_version("tensorfold-native 1.2.6").as_deref(), Some("1.2.6"));
        assert_eq!(first_version("version: 6240 (abcdef)").as_deref(), Some("6240"));
        assert_eq!(first_version("0.6.3+d76d468a.pa62586").as_deref(), Some("0.6.3"));
    }
}
