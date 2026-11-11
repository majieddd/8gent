//! Agent harnesses: the programs that drive a model with tools (Hermes, opencode, Claude Code, kr8).
//!
//! This is the first of three levels, "configure": 8gent knows which harnesses are on this computer,
//! whether each one is pointed at this endpoint, and can point it here. Running a harness and
//! serving one as an endpoint come later.
//!
//! A harness is a TOML description, like a loader. What cannot be a description is how its settings
//! file is written: each harness has its own format, so each kind of connection is code here, in two
//! parts: pure functions over the file's text (tested with the awkward cases), and a thin layer that
//! reads the file, keeps a copy of it, and writes the result in one step.
//!
//! Rules for touching someone's settings file:
//! - add or change only 8gent's own entry; never the harness's default model
//! - keep a copy first (`<data>/backups/harnesses/<id>/`), and say where it is
//! - when the file is not in the shape this code understands, change nothing and say so

use crate::paths::{self, Paths};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const BUILTIN: &[(&str, &str)] = &[
    ("hermes", include_str!("../harnesses/hermes.toml")),
    ("opencode", include_str!("../harnesses/opencode.toml")),
    ("claude-code", include_str!("../harnesses/claude-code.toml")),
    ("kr8", include_str!("../harnesses/kr8.toml")),
];

/// The name of 8gent's own entry in a harness's settings.
pub const ENTRY: &str = "8gent";

#[derive(Clone, Debug, Deserialize)]
pub struct Harness {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub homepage: String,
    #[serde(default)]
    pub detect: Detect,
    #[serde(default)]
    pub connect: Connect,
    #[serde(default)]
    pub run: Run,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Detect {
    /// Names looked up on the PATH, or paths (`~/` allowed). The first one found is the program.
    #[serde(default)]
    pub programs: Vec<String>,
    #[serde(default)]
    pub version_args: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ConnectKind {
    /// Hermes: a `providers:` entry in `~/.hermes/config.yaml`.
    HermesYaml,
    /// opencode: a `provider` entry in its JSON config.
    OpencodeJson,
    /// The harness has its own way to choose an endpoint; 8gent does not write its settings.
    Own,
    /// Nothing to connect (the harness cannot use this endpoint).
    #[default]
    None,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Connect {
    #[serde(default)]
    pub kind: ConnectKind,
    /// The settings file 8gent reads and, on connect, changes.
    #[serde(default)]
    pub file: String,
    /// Shown instead of a Connect button when there is nothing to write.
    #[serde(default)]
    pub note: String,
    /// How to use the harness with 8gent once connected, as a command to copy.
    #[serde(default, rename = "use")]
    pub usage: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Run {
    /// How the harness starts as an Agent Client Protocol agent (for running it; not used yet).
    #[serde(default)]
    pub acp: Vec<String>,
}

/// Built-in harnesses, then the files in `<home>/harnesses` (same id replaces, new id adds).
pub fn load_all(paths: &Paths) -> Result<Vec<Harness>, String> {
    let mut by_id: BTreeMap<String, Harness> = BTreeMap::new();
    let mut order: Vec<String> = vec![];
    for (id, text) in BUILTIN {
        let h: Harness = toml::from_str(text).map_err(|e| format!("built-in harness {id}: {e}"))?;
        order.push(h.id.clone());
        by_id.insert(h.id.clone(), h);
    }
    if let Ok(entries) = std::fs::read_dir(paths.home.join("harnesses")) {
        let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "toml")).collect();
        files.sort();
        for file in files {
            let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            let h: Harness = toml::from_str(&text).map_err(|e| format!("{}: {e}", file.display()))?;
            if !by_id.contains_key(&h.id) {
                order.push(h.id.clone());
            }
            by_id.insert(h.id.clone(), h);
        }
    }
    Ok(order.into_iter().filter_map(|id| by_id.remove(&id)).collect())
}

// ------------------------------------------------------------------------------------- this endpoint

/// What a harness is pointed at when it is pointed at this 8gent.
#[derive(Clone, Debug)]
pub struct Endpoint {
    /// The port 8gent writes into a harness's settings.
    pub port: u16,
    /// Every port this 8gent answers on (its own and the extra ones).
    pub ports: Vec<u16>,
    /// The model name that follows 8gent's own choice.
    pub served_name: String,
    /// The window the selected model is loaded with, when one is selected.
    pub context: Option<u32>,
}

impl Endpoint {
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }

    /// Whether `url` is an address of this 8gent on this computer.
    pub fn is_this(&self, url: &str) -> bool {
        let rest = url.trim().trim_matches(|c| c == '"' || c == '\'');
        let Some(rest) = rest.strip_prefix("http://").or_else(|| rest.strip_prefix("https://")) else {
            return false;
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        if authority.contains('@') {
            return false;
        }
        let (host, port) = match authority.strip_prefix('[') {
            Some(v6) => match v6.split_once("]:") {
                Some((h, p)) => (h, p),
                None => return false,
            },
            None => match authority.rsplit_once(':') {
                Some((h, p)) => (h, p),
                None => return false,
            },
        };
        let local = matches!(host.to_ascii_lowercase().as_str(), "127.0.0.1" | "localhost" | "::1" | "0.0.0.0");
        local && port.parse::<u16>().is_ok_and(|p| p == self.port || self.ports.contains(&p))
    }
}

// --------------------------------------------------------------------------------------------- Hermes

/// One entry of the `providers:` mapping in Hermes's YAML: its key and the lines it spans.
#[derive(Debug, PartialEq)]
struct YamlEntry {
    key: String,
    base_url: Option<String>,
    /// Line numbers: the entry's own line, and one past its last line.
    start: usize,
    end: usize,
}

/// The `providers:` block of a Hermes config, read without a YAML library: the block style Hermes
/// itself writes. Anything else (a flow mapping, tabs, an anchor) is refused, not guessed at.
#[derive(Debug, PartialEq)]
struct YamlProviders {
    /// The line of `providers:` itself; `None` when the file has no such key.
    key_line: Option<usize>,
    /// `providers: {}` on one line.
    empty_inline: bool,
    indent: usize,
    entries: Vec<YamlEntry>,
    /// One past the last line that belongs to the block.
    end: usize,
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

fn is_blank_or_comment(line: &str) -> bool {
    let t = line.trim();
    t.is_empty() || t.starts_with('#')
}

fn unquote(value: &str) -> String {
    let v = value.trim();
    // a comment after a plain value
    let v = if v.starts_with('"') || v.starts_with('\'') { v } else { v.split(" #").next().unwrap_or(v).trim() };
    v.trim_matches(|c| c == '"' || c == '\'').to_string()
}

fn yaml_key(line: &str) -> Option<(String, String)> {
    let t = line.trim_start_matches(' ');
    let (key, rest) = if let Some(q) = t.strip_prefix('"') {
        let (k, r) = q.split_once('"')?;
        (k.to_string(), r.strip_prefix(':')?)
    } else if let Some(q) = t.strip_prefix('\'') {
        let (k, r) = q.split_once('\'')?;
        (k.to_string(), r.strip_prefix(':')?)
    } else {
        let (k, r) = t.split_once(':')?;
        if k.is_empty() || k.contains([' ', '{', '[', '#']) {
            return None;
        }
        (k.to_string(), r)
    };
    if !(rest.is_empty() || rest.starts_with(' ')) {
        return None;
    }
    Some((key, rest.trim().to_string()))
}

fn yaml_providers(lines: &[&str]) -> Result<YamlProviders, String> {
    let odd = |what: &str| format!("its providers section is written in a way 8gent does not edit ({what})");
    let Some(key_line) = lines.iter().position(|l| indent_of(l) == 0 && yaml_key(l).is_some_and(|(k, _)| k == "providers")) else {
        return Ok(YamlProviders { key_line: None, empty_inline: false, indent: 2, entries: vec![], end: lines.len() });
    };
    let after = yaml_key(lines[key_line]).map(|(_, rest)| rest).unwrap_or_default();
    let after = after.split(" #").next().unwrap_or("").trim().to_string();
    let after = if after.starts_with('#') { String::new() } else { after };
    if after == "{}" || after == "null" || after == "~" {
        return Ok(YamlProviders { key_line: Some(key_line), empty_inline: true, indent: 2, entries: vec![], end: key_line + 1 });
    }
    if !after.is_empty() {
        return Err(odd("everything on one line"));
    }
    // the block: every following line until the next line that starts at the left edge
    let mut end = key_line + 1;
    while end < lines.len() {
        let line = lines[end];
        if line.trim_start_matches(' ').starts_with('\t') {
            return Err(odd("tabs"));
        }
        if !(is_blank_or_comment(line) || indent_of(line) > 0) {
            break;
        }
        end += 1;
    }
    // blank lines and comments at the end belong to what follows
    while end > key_line + 1 && is_blank_or_comment(lines[end - 1]) {
        end -= 1;
    }
    let body: Vec<usize> = (key_line + 1..end).filter(|i| !is_blank_or_comment(lines[*i])).collect();
    let Some(first) = body.first() else {
        return Ok(YamlProviders { key_line: Some(key_line), empty_inline: false, indent: 2, entries: vec![], end });
    };
    let indent = indent_of(lines[*first]);
    if lines[*first].trim_start().starts_with('-') {
        return Err(odd("a list"));
    }
    let mut entries: Vec<YamlEntry> = vec![];
    for i in body {
        let depth = indent_of(lines[i]);
        if depth < indent {
            return Err(odd("uneven indentation"));
        }
        if depth == indent {
            let Some((key, rest)) = yaml_key(lines[i]) else {
                return Err(odd("an entry that is not `name:`"));
            };
            if !rest.is_empty() && !rest.starts_with('#') {
                return Err(odd("an entry on one line"));
            }
            if let Some(last) = entries.last_mut() {
                last.end = i;
            }
            entries.push(YamlEntry { key, base_url: None, start: i, end });
        } else if let Some(last) = entries.last_mut() {
            if let Some((k, v)) = yaml_key(lines[i]) {
                if k == "base_url" && last.base_url.is_none() {
                    last.base_url = Some(unquote(&v));
                }
            }
        }
    }
    // an entry ends where the next begins, minus blank lines and comments that precede that one
    for e in &mut entries {
        while e.end > e.start + 1 && is_blank_or_comment(lines[e.end - 1]) {
            e.end -= 1;
        }
    }
    Ok(YamlProviders { key_line: Some(key_line), empty_inline: false, indent, entries, end })
}

/// The provider entries of a Hermes config that point at this endpoint, by name.
pub fn hermes_linked(text: &str, ep: &Endpoint) -> Result<Vec<String>, String> {
    let lines: Vec<&str> = text.lines().collect();
    Ok(yaml_providers(&lines)?.entries.into_iter().filter(|e| e.base_url.as_deref().is_some_and(|u| ep.is_this(u))).map(|e| e.key).collect())
}

fn hermes_entry(ep: &Endpoint, indent: usize) -> Vec<String> {
    let a = " ".repeat(indent);
    let b = " ".repeat(indent * 2);
    let c = " ".repeat(indent * 3);
    let name = &ep.served_name;
    vec![
        format!("{a}{ENTRY}:"),
        format!("{b}name: 8gent"),
        format!("{b}base_url: {}", ep.base_url()),
        format!("{b}model: {name}"),
        format!("{b}discover_models: true"),
        format!("{b}models:"),
        format!("{c}{name}: {{}}"),
    ]
}

fn join_lines(lines: Vec<String>, original: &str) -> String {
    let mut out = lines.join("\n");
    if original.ends_with('\n') || original.is_empty() {
        out.push('\n');
    }
    out
}

/// A Hermes config with 8gent's provider entry in it, pointing at this endpoint. Returns the text
/// unchanged when the entry is already right.
pub fn hermes_connect(text: &str, ep: &Endpoint) -> Result<String, String> {
    let lines: Vec<&str> = text.lines().collect();
    let p = yaml_providers(&lines)?;
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    match (p.key_line, p.entries.iter().find(|e| e.key == ENTRY)) {
        (_, Some(e)) if e.base_url.as_deref().is_some_and(|u| ep.is_this(u)) => return Ok(text.to_string()),
        // an entry of that name pointing somewhere else: it becomes this endpoint's
        (_, Some(e)) => {
            out.splice(e.start..e.end, hermes_entry(ep, p.indent));
        }
        (None, None) => {
            if out.last().is_some_and(|l| !l.trim().is_empty()) {
                out.push(String::new());
            }
            out.push("providers:".into());
            out.extend(hermes_entry(ep, p.indent));
        }
        (Some(k), None) if p.empty_inline => {
            let mut block = vec!["providers:".to_string()];
            block.extend(hermes_entry(ep, p.indent));
            out.splice(k..k + 1, block);
        }
        (Some(_), None) => {
            out.splice(p.end..p.end, hermes_entry(ep, p.indent));
        }
    }
    Ok(join_lines(out, text))
}

/// The provider Hermes uses by default (`model.provider`), when the file says.
fn hermes_default_provider(lines: &[&str]) -> Option<String> {
    let start = lines.iter().position(|l| indent_of(l) == 0 && yaml_key(l).is_some_and(|(k, _)| k == "model"))?;
    lines[start + 1..]
        .iter()
        .take_while(|l| is_blank_or_comment(l) || indent_of(l) > 0)
        .filter_map(|l| yaml_key(l))
        .find(|(k, _)| k == "provider")
        .map(|(_, v)| unquote(&v))
}

/// A Hermes config without 8gent's entry. Only the entry named `8gent` is removed, and only when it
/// points here; entries the person made themselves stay. Refused while it is Hermes's default.
pub fn hermes_disconnect(text: &str, ep: &Endpoint) -> Result<String, String> {
    let lines: Vec<&str> = text.lines().collect();
    let p = yaml_providers(&lines)?;
    let Some(e) = p.entries.iter().find(|e| e.key == ENTRY && e.base_url.as_deref().is_some_and(|u| ep.is_this(u))) else {
        return Ok(text.to_string());
    };
    if hermes_default_provider(&lines).as_deref() == Some(ENTRY) {
        return Err("Hermes uses 8gent as its default model. Choose another default first (hermes model), then disconnect.".into());
    }
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    out.drain(e.start..e.end);
    if p.entries.len() == 1 {
        // the mapping would be left without entries, which YAML reads as null
        if let Some(k) = p.key_line {
            out[k] = "providers: {}".into();
        }
    }
    Ok(join_lines(out, text))
}

// ------------------------------------------------------------------------------------------- opencode

/// The provider entries of an opencode config that point at this endpoint, by name.
pub fn opencode_linked(config: &Value, ep: &Endpoint) -> Vec<String> {
    let Some(providers) = config.get("provider").and_then(Value::as_object) else {
        return vec![];
    };
    providers
        .iter()
        .filter(|(_, p)| p.pointer("/options/baseURL").and_then(Value::as_str).is_some_and(|u| ep.is_this(u)))
        .map(|(k, _)| k.clone())
        .collect()
}

fn opencode_entry(ep: &Endpoint) -> Value {
    let mut model = json!({"name": format!("{} (the model chosen in 8gent)", ep.served_name)});
    if let Some(context) = ep.context.filter(|c| *c >= 2048) {
        // opencode plans with these; a quarter of the window is left for the answer
        model["limit"] = json!({"context": context, "output": context / 4});
    }
    json!({
        "npm": "@ai-sdk/openai-compatible",
        "name": "8gent",
        "options": {"baseURL": ep.base_url()},
        "models": {ep.served_name.clone(): model},
    })
}

/// An opencode config with 8gent's provider entry in it. Everything else is kept as it was.
pub fn opencode_connect(mut config: Value, ep: &Endpoint) -> Result<Value, String> {
    let root = config.as_object_mut().ok_or("its settings file is not a JSON object")?;
    let providers = root.entry("provider").or_insert_with(|| json!({}));
    let providers = providers.as_object_mut().ok_or("its \"provider\" setting is not an object")?;
    providers.insert(ENTRY.to_string(), opencode_entry(ep));
    Ok(config)
}

/// An opencode config without 8gent's entry (only when that entry points here). Refused while a
/// default model of opencode's is one of 8gent's.
pub fn opencode_disconnect(mut config: Value, ep: &Endpoint) -> Result<Value, String> {
    if !opencode_linked(&config, ep).iter().any(|k| k == ENTRY) {
        return Ok(config);
    }
    for key in ["model", "small_model"] {
        if config.get(key).and_then(Value::as_str).is_some_and(|m| m.starts_with("8gent/")) {
            return Err(format!("opencode's \"{key}\" setting is an 8gent model. Choose another one first, then disconnect."));
        }
    }
    if let Some(root) = config.as_object_mut() {
        let now_empty = root.get_mut("provider").and_then(Value::as_object_mut).is_some_and(|p| {
            p.remove(ENTRY);
            p.is_empty()
        });
        if now_empty {
            root.remove("provider");
        }
    }
    Ok(config)
}

// ------------------------------------------------------------------------------------------ the files

/// How a harness stands with this endpoint.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Link {
    /// Pointed here, through these entries of its settings.
    Connected { entries: Vec<String> },
    NotConnected,
    /// Its settings file is not in a shape 8gent reads; nothing will be written to it.
    Unreadable { why: String },
    /// The harness chooses its endpoint its own way, or cannot use this one.
    NotByEightgent { why: String },
}

fn read_settings(file: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(file) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", file.display())),
    }
}

pub fn settings_file(h: &Harness, paths: &Paths) -> PathBuf {
    PathBuf::from(paths::expand(&h.connect.file, paths))
}

pub fn link(h: &Harness, ep: &Endpoint, paths: &Paths) -> Link {
    let file = settings_file(h, paths);
    let linked = |entries: Vec<String>| if entries.is_empty() { Link::NotConnected } else { Link::Connected { entries } };
    match h.connect.kind {
        ConnectKind::None | ConnectKind::Own => Link::NotByEightgent { why: h.connect.note.clone() },
        ConnectKind::HermesYaml => match read_settings(&file) {
            Ok(None) => Link::NotConnected,
            Ok(Some(text)) => hermes_linked(&text, ep).map(linked).unwrap_or_else(|why| Link::Unreadable { why }),
            Err(why) => Link::Unreadable { why },
        },
        ConnectKind::OpencodeJson => match read_settings(&file) {
            Ok(None) => Link::NotConnected,
            Ok(Some(text)) if text.trim().is_empty() => Link::NotConnected,
            Ok(Some(text)) => match serde_json::from_str::<Value>(&text) {
                Ok(config) => linked(opencode_linked(&config, ep)),
                Err(e) => Link::Unreadable { why: format!("its settings file is not plain JSON ({e})") },
            },
            Err(why) => Link::Unreadable { why },
        },
    }
}

/// What a connect or disconnect did.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Written {
    pub changed: bool,
    pub file: String,
    /// The copy of the file as it was, when it existed and was changed.
    pub backup: Option<String>,
}

fn stamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format!("{secs}")
}

/// Replace `file` with `text` in one step, keeping a copy of what was there under the data folder.
fn write_keeping_a_copy(h: &Harness, file: &Path, before: Option<&str>, text: &str, paths: &Paths) -> Result<Written, String> {
    if before == Some(text) {
        return Ok(Written { changed: false, file: file.to_string_lossy().into(), backup: None });
    }
    let mut backup = None;
    if before.is_some() {
        let dir = paths.home.join("backups").join("harnesses").join(&h.id);
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "settings".into());
        let copy = dir.join(format!("{}-{name}", stamp()));
        std::fs::copy(file, &copy).map_err(|e| format!("could not keep a copy of {}: {e}", file.display()))?;
        backup = Some(copy.to_string_lossy().to_string());
    } else if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let temp = file.with_extension(format!("8gent-{}.tmp", std::process::id()));
    std::fs::write(&temp, text).map_err(|e| format!("{}: {e}", temp.display()))?;
    if let Ok(meta) = std::fs::metadata(file) {
        let _ = std::fs::set_permissions(&temp, meta.permissions());
    }
    std::fs::rename(&temp, file).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        format!("{}: {e}", file.display())
    })?;
    Ok(Written { changed: true, file: file.to_string_lossy().into(), backup })
}

fn to_pretty(config: &Value) -> String {
    let mut text = serde_json::to_string_pretty(config).unwrap_or_else(|_| "{}".into());
    text.push('\n');
    text
}

fn not_writable(h: &Harness) -> String {
    if h.connect.note.is_empty() { format!("8gent does not write {}'s settings.", h.name) } else { h.connect.note.clone() }
}

/// Point the harness at this endpoint.
pub fn connect(h: &Harness, ep: &Endpoint, paths: &Paths) -> Result<Written, String> {
    let file = settings_file(h, paths);
    let before = read_settings(&file)?;
    let refuse = |why: String| format!("{}: {why}. Nothing was changed in {}.", h.name, file.display());
    match h.connect.kind {
        ConnectKind::None | ConnectKind::Own => Err(not_writable(h)),
        ConnectKind::HermesYaml => {
            let text = hermes_connect(before.as_deref().unwrap_or(""), ep).map_err(refuse)?;
            write_keeping_a_copy(h, &file, before.as_deref(), &text, paths)
        }
        ConnectKind::OpencodeJson => {
            let config = match before.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
                None => json!({"$schema": "https://opencode.ai/config.json"}),
                Some(text) => serde_json::from_str(text).map_err(|e| refuse(format!("its settings file is not plain JSON ({e})")))?,
            };
            // already right: leave the file exactly as it is, formatting included
            if config.pointer(&format!("/provider/{ENTRY}")) == Some(&opencode_entry(ep)) {
                return Ok(Written { changed: false, file: file.to_string_lossy().into(), backup: None });
            }
            let config = opencode_connect(config, ep).map_err(refuse)?;
            write_keeping_a_copy(h, &file, before.as_deref(), &to_pretty(&config), paths)
        }
    }
}

/// Take 8gent's entry out of the harness's settings again.
pub fn disconnect(h: &Harness, ep: &Endpoint, paths: &Paths) -> Result<Written, String> {
    let file = settings_file(h, paths);
    let Some(before) = read_settings(&file)? else {
        return Ok(Written { changed: false, file: file.to_string_lossy().into(), backup: None });
    };
    let refuse = |why: String| format!("{}: {why}. Nothing was changed in {}.", h.name, file.display());
    match h.connect.kind {
        ConnectKind::None | ConnectKind::Own => Err(not_writable(h)),
        ConnectKind::HermesYaml => {
            let text = hermes_disconnect(&before, ep).map_err(refuse)?;
            write_keeping_a_copy(h, &file, Some(&before), &text, paths)
        }
        ConnectKind::OpencodeJson => {
            let config: Value = serde_json::from_str(&before).map_err(|e| refuse(format!("its settings file is not plain JSON ({e})")))?;
            if !opencode_linked(&config, ep).iter().any(|k| k == ENTRY) {
                return Ok(Written { changed: false, file: file.to_string_lossy().into(), backup: None });
            }
            let config = opencode_disconnect(config, ep).map_err(|why| format!("{}: {why}", h.name))?;
            write_keeping_a_copy(h, &file, Some(&before), &to_pretty(&config), paths)
        }
    }
}

// ------------------------------------------------------------------------------------------ detection

/// The harness's program on this computer, when it is installed.
pub fn program(h: &Harness, paths: &Paths) -> Option<PathBuf> {
    h.detect.programs.iter().find_map(|p| {
        if p.starts_with('~') || p.contains(std::path::MAIN_SEPARATOR) {
            paths::find_program(std::slice::from_ref(p), None, paths)
        } else {
            // a bare name: wherever the PATH has it (a login service's PATH is short, so the
            // descriptions also name the usual places)
            paths::find_program(&[], Some(p), paths)
        }
    })
}

/// Run `program args` and give what it printed, waiting at most `limit`.
fn output_within(program: &Path, args: &[String], limit: std::time::Duration) -> Option<String> {
    use std::process::{Command, Stdio};
    let mut child = Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().ok()?;
    let until = std::time::Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() >= until => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(40)),
            Err(_) => return None,
        }
    }
    let out = child.wait_with_output().ok()?;
    Some(format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

/// The harness's version, asked once per program file (asking starts the program, which for some
/// takes a second or two).
pub fn version(h: &Harness, program: &Path) -> Option<String> {
    if h.detect.version_args.is_empty() {
        return None;
    }
    static SEEN: OnceLock<Mutex<HashMap<(PathBuf, Option<std::time::SystemTime>), Option<String>>>> = OnceLock::new();
    let key = (program.to_path_buf(), std::fs::metadata(program).and_then(|m| m.modified()).ok());
    if let Some(known) = SEEN.get_or_init(Default::default).lock().unwrap().get(&key) {
        return known.clone();
    }
    let found = output_within(program, &h.detect.version_args, std::time::Duration::from_secs(8)).and_then(|text| crate::engine::version_in(&text));
    SEEN.get_or_init(Default::default).lock().unwrap().insert(key, found.clone());
    found
}

/// Everything the app and the command line show about one harness.
pub fn describe(h: &Harness, ep: &Endpoint, paths: &Paths) -> Value {
    let program = program(h, paths);
    let version = program.as_deref().and_then(|p| version(h, p));
    let link = link(h, ep, paths);
    let writable = matches!(h.connect.kind, ConnectKind::HermesYaml | ConnectKind::OpencodeJson);
    json!({
        "id": h.id,
        "name": h.name,
        "summary": h.summary,
        "homepage": h.homepage,
        "installed": program.is_some(),
        "program": program.map(|p| p.to_string_lossy().to_string()),
        "version": version,
        "link": link,
        "can_connect": writable,
        // "own": the harness chooses its endpoint itself; "none": it cannot use this endpoint
        "connect_kind": match h.connect.kind { ConnectKind::HermesYaml | ConnectKind::OpencodeJson => "settings", ConnectKind::Own => "own", ConnectKind::None => "none" },
        "settings_file": if writable { Some(settings_file(h, paths).to_string_lossy().to_string()) } else { None },
        "use": h.connect.usage,
        "runs_as_agent": !h.run.acp.is_empty(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep() -> Endpoint {
        Endpoint { port: 8436, ports: vec![8436, 8585, 8484], served_name: "8gent".into(), context: Some(96_000) }
    }

    #[test]
    fn the_built_in_harnesses_read() {
        let all = load_all(&Paths { home: std::env::temp_dir().join("8gent-no-such-folder") }).unwrap();
        let ids: Vec<&str> = all.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(ids, ["hermes", "opencode", "claude-code", "kr8"]);
        assert_eq!(all[0].connect.kind, ConnectKind::HermesYaml);
        assert_eq!(all[1].connect.kind, ConnectKind::OpencodeJson);
        assert_eq!(all[2].connect.kind, ConnectKind::None);
        assert_eq!(all[3].connect.kind, ConnectKind::Own);
        assert!(!all[2].connect.note.is_empty() && !all[3].connect.note.is_empty());
    }

    #[test]
    fn only_this_computers_8gent_counts_as_this_endpoint() {
        let ep = ep();
        for yes in ["http://127.0.0.1:8436/v1", "http://localhost:8436/v1/", "http://127.0.0.1:8585/v1", " 'http://127.0.0.1:8484/v1' ", "http://[::1]:8436/v1", "HTTP://LOCALHOST:8436".to_lowercase().as_str()] {
            assert!(ep.is_this(yes), "{yes}");
        }
        for no in [
            "http://127.0.0.1:11434/v1",
            "http://192.168.1.20:8436/v1",
            "http://127.0.0.1.example.com:8436/v1",
            "http://example.com@127.0.0.1:8436/v1",
            "http://127.0.0.1/v1",
            "https://openrouter.ai/api/v1",
            "127.0.0.1:8436",
            "http://127.0.0.1:84360/v1",
            "",
        ] {
            assert!(!ep.is_this(no), "{no}");
        }
    }

    const HERMES: &str = "model:\n  default: z-ai/glm-5.3-flash\n  provider: openrouter\n  base_url: 'https://openrouter.ai/api/v1'\ntoolsets:\n  - hermes-cli\nproviders:\n  local:\n    name: Mine\n    base_url: http://127.0.0.1:11434/v1\n    models:\n      a: {}\n\n# what follows is not a provider\nagent:\n  max_turns: 150\n";

    #[test]
    fn hermes_gets_one_entry_and_nothing_else_moves() {
        let out = hermes_connect(HERMES, &ep()).unwrap();
        let added = "  8gent:\n    name: 8gent\n    base_url: http://127.0.0.1:8436/v1\n    model: 8gent\n    discover_models: true\n    models:\n      8gent: {}\n";
        assert_eq!(out, HERMES.replace("      a: {}\n", &format!("      a: {{}}\n{added}")));
        assert_eq!(hermes_linked(&out, &ep()).unwrap(), ["8gent"]);
        // the default model is not touched
        assert!(out.starts_with("model:\n  default: z-ai/glm-5.3-flash\n  provider: openrouter\n"));
        // a second connect changes nothing
        assert_eq!(hermes_connect(&out, &ep()).unwrap(), out);
        // and disconnecting gives the file back, byte for byte
        assert_eq!(hermes_disconnect(&out, &ep()).unwrap(), HERMES);
    }

    #[test]
    fn hermes_without_a_providers_section_gets_one() {
        for (before, after_start) in [
            ("model:\n  default: m\n", "model:\n  default: m\n\nproviders:\n  8gent:\n"),
            ("", "providers:\n  8gent:\n"),
            ("model:\n  default: m\nproviders: {}\nagent:\n  x: 1\n", "model:\n  default: m\nproviders:\n  8gent:\n"),
            ("providers:\nagent:\n  x: 1\n", "providers:\n  8gent:\n    name: 8gent\n"),
        ] {
            let out = hermes_connect(before, &ep()).unwrap();
            assert!(out.starts_with(after_start), "{before:?} -> {out:?}");
            assert_eq!(hermes_linked(&out, &ep()).unwrap(), ["8gent"], "{before:?}");
            assert!(out.ends_with('\n'));
            if before.contains("agent:") {
                assert!(out.ends_with("agent:\n  x: 1\n"), "{out:?}");
            }
        }
    }

    #[test]
    fn hermes_entries_the_person_made_are_seen_and_kept() {
        let mine = "providers:\n  uzu:\n    name: uzu Home\n    base_url: \"http://127.0.0.1:8585/v1\"  # the old port\n    model: uzu\n  other:\n    base_url: https://example.com/v1\n";
        assert_eq!(hermes_linked(mine, &ep()).unwrap(), ["uzu"]);
        // nothing named 8gent: disconnect leaves it all alone
        assert_eq!(hermes_disconnect(mine, &ep()).unwrap(), mine);
        let both = hermes_connect(mine, &ep()).unwrap();
        assert_eq!(hermes_linked(&both, &ep()).unwrap(), ["uzu", "8gent"]);
        assert_eq!(hermes_disconnect(&both, &ep()).unwrap(), mine);
    }

    #[test]
    fn an_8gent_entry_that_points_elsewhere_is_made_to_point_here_and_never_removed() {
        let stale = "providers:\n  8gent:\n    name: old\n    base_url: http://10.0.0.5:9999/v1\n    extra: kept-out\n  z:\n    base_url: http://x/v1\n";
        // not this endpoint: disconnect does not remove someone else's 8gent
        assert_eq!(hermes_disconnect(stale, &ep()).unwrap(), stale);
        let out = hermes_connect(stale, &ep()).unwrap();
        assert_eq!(hermes_linked(&out, &ep()).unwrap(), ["8gent"]);
        assert!(!out.contains("10.0.0.5") && out.contains("  z:\n    base_url: http://x/v1\n"));
    }

    #[test]
    fn hermes_is_not_disconnected_while_8gent_is_its_default() {
        let text = "model:\n  default: 8gent\n  provider: 8gent\nproviders:\n  8gent:\n    base_url: http://127.0.0.1:8436/v1\n";
        let err = hermes_disconnect(text, &ep()).unwrap_err();
        assert!(err.contains("default"), "{err}");
        // the only entry going leaves an empty mapping, not a null
        let alone = "model:\n  provider: openrouter\nproviders:\n  8gent:\n    base_url: http://127.0.0.1:8436/v1\nagent:\n  x: 1\n";
        assert_eq!(hermes_disconnect(alone, &ep()).unwrap(), "model:\n  provider: openrouter\nproviders: {}\nagent:\n  x: 1\n");
    }

    #[test]
    fn hermes_files_in_other_shapes_are_refused_not_guessed() {
        for odd in [
            "providers: {local: {base_url: http://127.0.0.1:8436/v1}}\n",
            "providers:\n\t8gent:\n\t\tbase_url: x\n",
            "providers:\n  - name: a\n    base_url: x\n",
            "providers:\n  a: {base_url: http://127.0.0.1:8436/v1}\n",
            "providers:\n    a:\n      base_url: x\n  b:\n    base_url: y\n",
        ] {
            assert!(hermes_linked(odd, &ep()).is_err(), "{odd:?}");
            assert!(hermes_connect(odd, &ep()).is_err(), "{odd:?}");
            assert!(hermes_disconnect(odd, &ep()).is_err(), "{odd:?}");
        }
        // four-space files are edited in their own indentation
        let four = "providers:\n    a:\n        base_url: http://x/v1\n";
        let out = hermes_connect(four, &ep()).unwrap();
        assert!(out.contains("\n    8gent:\n        name: 8gent\n        base_url: http://127.0.0.1:8436/v1\n"), "{out}");
        assert!(out.contains("            8gent: {}\n"));
    }

    #[test]
    fn opencode_gets_a_provider_and_keeps_everything_else_in_order() {
        let before: Value = serde_json::from_str(r#"{"mcp":{"pencil":{"type":"local","enabled":true}},"$schema":"https://opencode.ai/config.json","theme":"x"}"#).unwrap();
        let after = opencode_connect(before.clone(), &ep()).unwrap();
        assert_eq!(after["mcp"], before["mcp"]);
        assert_eq!(after["theme"], "x");
        assert_eq!(after["provider"]["8gent"]["npm"], "@ai-sdk/openai-compatible");
        assert_eq!(after["provider"]["8gent"]["options"]["baseURL"], "http://127.0.0.1:8436/v1");
        assert_eq!(after["provider"]["8gent"]["models"]["8gent"]["limit"], json!({"context": 96000, "output": 24000}));
        // keys stay where the person had them; the new one goes last
        let keys: Vec<&String> = after.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["mcp", "$schema", "theme", "provider"]);
        assert_eq!(opencode_linked(&after, &ep()), ["8gent"]);
        assert_eq!(opencode_disconnect(after, &ep()).unwrap(), before);
    }

    #[test]
    fn opencode_other_providers_stay_and_a_default_on_8gent_blocks_disconnect() {
        let before = json!({"provider": {"mine": {"options": {"baseURL": "http://localhost:8585/v1"}}, "far": {"options": {"baseURL": "https://example.com/v1"}}}});
        assert_eq!(opencode_linked(&before, &ep()), ["mine"]);
        assert_eq!(opencode_disconnect(before.clone(), &ep()).unwrap(), before, "nothing named 8gent");
        let mut after = opencode_connect(before.clone(), &ep()).unwrap();
        assert_eq!(opencode_disconnect(after.clone(), &ep()).unwrap(), before);
        after["model"] = json!("8gent/8gent");
        assert!(opencode_disconnect(after, &ep()).unwrap_err().contains("\"model\""));

        // no window known: no limit is invented
        let mut unknown = ep();
        unknown.context = None;
        let after = opencode_connect(json!({}), &unknown).unwrap();
        assert!(after["provider"]["8gent"]["models"]["8gent"].get("limit").is_none());
        // settings that are not what they should be are refused
        assert!(opencode_connect(json!([]), &ep()).is_err());
        assert!(opencode_connect(json!({"provider": "x"}), &ep()).is_err());
    }

    fn scratch(name: &str) -> (Paths, PathBuf) {
        let root = std::env::temp_dir().join(format!("8gent-harness-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("data")).unwrap();
        (Paths { home: root.join("data") }, root)
    }

    fn harness(kind: &str, file: &Path) -> Harness {
        toml::from_str(&format!("id = \"h\"\nname = \"H\"\n[connect]\nkind = \"{kind}\"\nfile = \"{}\"\n", file.display())).unwrap()
    }

    #[test]
    fn connecting_keeps_a_copy_and_writes_once() {
        let (paths, root) = scratch("hermes");
        let file = root.join("config.yaml");
        std::fs::write(&file, HERMES).unwrap();
        let h = harness("hermes-yaml", &file);
        assert_eq!(link(&h, &ep(), &paths), Link::NotConnected);

        let first = connect(&h, &ep(), &paths).unwrap();
        assert!(first.changed);
        let copy = first.backup.expect("a copy of the file as it was");
        assert_eq!(std::fs::read_to_string(&copy).unwrap(), HERMES);
        assert!(copy.starts_with(&*paths.home.to_string_lossy()));
        assert_eq!(link(&h, &ep(), &paths), Link::Connected { entries: vec!["8gent".into()] });

        let second = connect(&h, &ep(), &paths).unwrap();
        assert!(!second.changed && second.backup.is_none(), "already connected: nothing written");

        let off = disconnect(&h, &ep(), &paths).unwrap();
        assert!(off.changed);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), HERMES);
        // no stray temporary files beside the settings
        let left: Vec<String> = std::fs::read_dir(&root).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        assert!(left.iter().all(|n| !n.ends_with(".tmp")), "{left:?}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_file_that_cannot_be_edited_is_left_exactly_as_it_is() {
        let (paths, root) = scratch("odd");
        let yaml = root.join("config.yaml");
        let odd = "providers: {a: {base_url: http://127.0.0.1:8436/v1}}\n";
        std::fs::write(&yaml, odd).unwrap();
        let h = harness("hermes-yaml", &yaml);
        assert!(matches!(link(&h, &ep(), &paths), Link::Unreadable { .. }));
        let err = connect(&h, &ep(), &paths).unwrap_err();
        assert!(err.contains("Nothing was changed"), "{err}");
        assert_eq!(std::fs::read_to_string(&yaml).unwrap(), odd);

        let jsonc = root.join("opencode.json");
        let with_comment = "{\n  // mine\n  \"theme\": \"x\"\n}\n";
        std::fs::write(&jsonc, with_comment).unwrap();
        let h = harness("opencode-json", &jsonc);
        assert!(matches!(link(&h, &ep(), &paths), Link::Unreadable { .. }));
        assert!(connect(&h, &ep(), &paths).is_err());
        assert_eq!(std::fs::read_to_string(&jsonc).unwrap(), with_comment);
        assert!(!paths.home.join("backups").exists(), "no copy is made of a file that is not changed");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn opencode_settings_that_do_not_exist_yet_are_made() {
        let (paths, root) = scratch("opencode");
        let file = root.join("deeper").join("opencode.json");
        let h = harness("opencode-json", &file);
        assert_eq!(link(&h, &ep(), &paths), Link::NotConnected);
        let done = connect(&h, &ep(), &paths).unwrap();
        assert!(done.changed && done.backup.is_none());
        let made: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(made["$schema"], "https://opencode.ai/config.json");
        assert_eq!(link(&h, &ep(), &paths), Link::Connected { entries: vec!["8gent".into()] });
        assert!(!connect(&h, &ep(), &paths).unwrap().changed);

        // a harness 8gent does not write for
        let own = harness("own", &file);
        assert!(connect(&own, &ep(), &paths).is_err());
        assert!(matches!(link(&own, &ep(), &paths), Link::NotByEightgent { .. }));
        let _ = std::fs::remove_dir_all(root);
    }
}
