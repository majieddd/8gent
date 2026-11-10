//! The model library: every model 8gent knows, each with one build per format it exists in.
//! "OxCoder-9B" is one model here even though it is on disk three times (GGUF for llama.cpp, MLX for
//! TensorFold, a converted folder for uzu); choosing a loader picks the build that loader can run.

use crate::paths::{self, Paths};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{BufReader, Read, Seek};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub name: String,
    #[serde(default, rename = "build")]
    pub builds: Vec<Build>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Build {
    /// "gguf", "mlx", "uzu", "mirai", or an attached server's own format ("ollama", "lmstudio"...).
    pub format: String,
    /// What the loader is given: a file, a folder, a Hugging Face repo or the server's model name.
    #[serde(rename = "ref")]
    pub reference: String,
    /// Only this loader can run the build (an attached server's models).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loader: Option<String>,
    /// The loader's variant that runs it (TensorFold's "tf06").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// The longest context the model was built for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_context: Option<u32>,
    /// Bytes of KV cache one token of context takes, for the memory estimate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kv_bytes_per_token: Option<u64>,
    /// Where 8gent found it: "lmstudio", "uzuhome", "tensorfoldhome", "manual", or a loader id.
    #[serde(default)]
    pub source: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Library {
    #[serde(default, rename = "model")]
    pub models: Vec<Model>,
}

impl Library {
    pub fn get(&self, id: &str) -> Option<&Model> {
        self.models.iter().find(|m| m.id == id)
    }

    /// Add a build, merging into a model with the same id. A second build of the same format and
    /// reference is dropped; of the same format from another place, kept only if nothing covers it yet.
    pub fn add(&mut self, id: &str, name: &str, build: Build) {
        let model = match self.models.iter_mut().find(|m| m.id == id) {
            Some(m) => m,
            None => {
                self.models.push(Model { id: id.to_string(), name: name.to_string(), builds: vec![] });
                self.models.last_mut().unwrap()
            }
        };
        let same = |b: &Build| b.format == build.format && (b.reference == build.reference || (b.loader == build.loader && b.quant == build.quant));
        if !model.builds.iter().any(same) {
            model.builds.push(build);
        }
    }

    pub fn sort(&mut self) {
        self.models.sort_by(|a, b| a.id.cmp(&b.id));
    }
}

/// Everything found on this computer's disks: the hand-written list, then (with `external`) the folders
/// of LM Studio and the earlier Home apps. Attached servers' models are added by the service, which asks them.
pub fn scan(paths: &Paths, external: bool) -> Library {
    let mut lib = Library::default();
    if let Ok(text) = std::fs::read_to_string(paths.models()) {
        match toml::from_str::<Library>(&text) {
            Ok(manual) => {
                for m in manual.models {
                    for mut b in m.builds {
                        if b.source.is_empty() {
                            b.source = "manual".into();
                        }
                        b.reference = paths::expand(&b.reference, paths);
                        lib.add(&m.id, &m.name, b);
                    }
                }
            }
            Err(e) => eprintln!("8gent: {}: {e}", paths.models().display()),
        }
    }
    if !external {
        lib.sort();
        return lib;
    }
    let home = paths::user_home();
    scan_tensorfold_home(&home.join("Library/Application Support/TensorFoldLauncher/models.conf"), &mut lib);
    scan_uzu_home(&home.join("Library/Application Support/UzuHome/backend/models.conf"), &mut lib);
    let lmstudio = std::env::var_os("EIGHTGENT_LMSTUDIO_MODELS").map(PathBuf::from).unwrap_or(home.join(".lmstudio/models"));
    scan_lmstudio(&lmstudio, &mut lib);
    lib.sort();
    lib
}

/// `<publisher>/<repo>/` folders: GGUF files (one build each, projector files skipped) and MLX folders.
pub fn scan_lmstudio(root: &Path, lib: &mut Library) {
    for publisher in subdirs(root) {
        for repo in subdirs(&publisher) {
            let repo_name = file_name(&repo);
            let (id, name) = (model_id(&repo_name), display_name(&repo_name));
            let mut files: Vec<PathBuf> = std::fs::read_dir(&repo).into_iter().flatten().flatten().map(|e| e.path()).collect();
            files.sort();
            let mut found_gguf = false;
            for f in &files {
                let fname = file_name(f);
                let lower = fname.to_lowercase();
                if !lower.ends_with(".gguf") || lower.starts_with("mmproj") {
                    continue;
                }
                found_gguf = true;
                let facts = gguf_facts(f);
                lib.add(&id, &name, Build {
                    format: "gguf".into(),
                    reference: f.to_string_lossy().into_owned(),
                    quant: gguf_quant(&fname),
                    size_bytes: std::fs::metadata(f).ok().map(|m| m.len()),
                    max_context: facts.as_ref().and_then(|x| x.context_length),
                    kv_bytes_per_token: facts.as_ref().and_then(|x| x.kv_bytes_per_token),
                    source: "lmstudio".into(),
                    ..Default::default()
                });
            }
            if !found_gguf && repo.join("config.json").is_file() && files.iter().any(|f| file_name(f).ends_with(".safetensors")) {
                let config = read_json(&repo.join("config.json"));
                let (max_context, kv) = hf_config_facts(&config);
                lib.add(&id, &name, Build {
                    format: "mlx".into(),
                    reference: repo.to_string_lossy().into_owned(),
                    quant: mlx_quant(&repo_name),
                    size_bytes: Some(files.iter().filter(|f| file_name(f).ends_with(".safetensors")).filter_map(|f| std::fs::metadata(f).ok()).map(|m| m.len()).sum()),
                    max_context,
                    kv_bytes_per_token: kv,
                    source: "lmstudio".into(),
                    ..Default::default()
                });
            }
        }
    }
}

/// TensorFold Home's list: `key|label|engine|Hugging Face repo`.
pub fn scan_tensorfold_home(conf: &Path, lib: &mut Library) {
    for fields in conf_lines(conf) {
        let [_key, _label, engine, repo] = fields.as_slice() else { continue };
        let repo_name = repo.rsplit('/').next().unwrap_or(repo);
        let folder = hf_snapshot(repo);
        let config = folder.as_ref().map(|d| read_json(&d.join("config.json"))).unwrap_or_default();
        let (max_context, kv) = hf_config_facts(&config);
        lib.add(&model_id(repo_name), &display_name(repo_name), Build {
            format: "mlx".into(),
            reference: repo.to_string(),
            variant: (engine == "tf06").then(|| "tf06".to_string()),
            quant: mlx_quant(repo_name),
            size_bytes: folder.as_ref().map(|d| dir_size(d, ".safetensors")).filter(|n| *n > 0),
            max_context,
            kv_bytes_per_token: kv,
            source: "tensorfoldhome".into(),
            ..Default::default()
        });
    }
}

/// uzu Home's list: `key|label|source|reference`, source `local` (a converted folder) or `mirai` (Mirai's registry).
pub fn scan_uzu_home(conf: &Path, lib: &mut Library) {
    for fields in conf_lines(conf) {
        let [key, label, source, reference] = fields.as_slice() else { continue };
        if source == "local" {
            let dir = PathBuf::from(reference);
            if !dir.join("config.json").is_file() {
                continue;
            }
            let folder = file_name(&dir);
            let (kv, longest) = lalamo_kv(&read_json(&dir.join("config.json")));
            let (spec_kv, _) = lalamo_kv(&read_json(&dir.join("speculator/config.json")));
            lib.add(&model_id(&folder), &display_name(&folder), Build {
                format: "uzu".into(),
                reference: reference.to_string(),
                size_bytes: Some(dir_size(&dir, ".safetensors") + dir_size(&dir.join("speculator"), ".safetensors")).filter(|n| *n > 0),
                max_context: longest,
                kv_bytes_per_token: (kv > 0).then_some(kv + spec_kv),
                source: "uzuhome".into(),
                ..Default::default()
            });
        } else {
            // alibaba:qwen3.5:9b:mirai:mirai-m:4 -> model "qwen3.5-9b", build "mirai-m"
            let parts: Vec<&str> = reference.split(':').collect();
            let (id, quant) = if parts.len() >= 5 { (format!("{}-{}", parts[1], parts[2]), Some(parts[4].to_string())) } else { (key.to_string(), None) };
            let name = label.split(" · ").next().unwrap_or(label).to_string();
            lib.add(&model_id(&id), &name, Build {
                format: "mirai".into(),
                reference: reference.to_string(),
                quant,
                source: "uzuhome".into(),
                ..Default::default()
            });
        }
    }
}

// ------------------------------------------------------------------------------------------- naming

/// One id for a model whatever format its folder is named for:
/// "ahmedandaloes/Qwythos-9B-v2-MLX-4bit", "Qwythos-9B-v2-GGUF" and "Qwythos-9B-v2" all give "qwythos-9b-v2".
pub fn model_id(name: &str) -> String {
    let base = name.rsplit('/').next().unwrap_or(name);
    let cleaned: String = base.chars().map(|c| if c == '_' || c == ' ' || c == ':' { '-' } else { c.to_ascii_lowercase() }).collect();
    let kept: Vec<&str> = cleaned.split('-').filter(|t| !t.is_empty() && !is_format_token(t)).collect();
    kept.join("-")
}

/// The folder name without the format words, case kept: "Qwythos-9B-v2-MLX-4bit" -> "Qwythos-9B-v2".
pub fn display_name(name: &str) -> String {
    let base = name.rsplit('/').next().unwrap_or(name);
    let kept: Vec<&str> = base.split(['-', '_']).filter(|t| !t.is_empty() && !is_format_token(&t.to_lowercase())).collect();
    kept.join("-")
}

fn is_format_token(t: &str) -> bool {
    matches!(t, "gguf" | "mlx" | "safetensors" | "bit" | "bf16" | "f16" | "fp16") || (t.ends_with("bit") && t[..t.len() - 3].chars().all(|c| c.is_ascii_digit()))
}

/// "Qwen3.5-0.8B-Q4_K_M.gguf" -> "Q4_K_M".
pub fn gguf_quant(file: &str) -> Option<String> {
    let stem = file.strip_suffix(".gguf").or(file.strip_suffix(".GGUF")).unwrap_or(file);
    stem.rsplit(['-', '.']).find(|t| {
        let u = t.to_uppercase();
        (u.starts_with('Q') || u.starts_with("IQ") || u.starts_with("UD") || u == "BF16" || u == "F16" || u == "F32") && u.chars().any(|c| c.is_ascii_digit())
    }).map(|t| t.to_string())
}

fn mlx_quant(repo_name: &str) -> Option<String> {
    repo_name.to_lowercase().split(['-', '_']).find(|t| t.ends_with("bit") && t.len() > 3).map(|t| format!("{}-bit", &t[..t.len() - 3]))
}

// ------------------------------------------------------------------------------------------- facts

fn read_json(path: &Path) -> serde_json::Value {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(serde_json::Value::Null)
}

/// A Hugging Face `config.json` (MLX builds): the longest context, and the KV bytes one token takes
/// in the layers that have full attention (Qwen3.5 mixes them with layers that keep no KV cache).
fn hf_config_facts(config: &serde_json::Value) -> (Option<u32>, Option<u64>) {
    let t = config.get("text_config").filter(|v| v.is_object()).unwrap_or(config);
    let n = |k: &str| t.get(k).and_then(|v| v.as_u64());
    let max_context = n("max_position_embeddings").map(|v| v as u32);
    let layers = n("num_hidden_layers").unwrap_or(0);
    let attention_layers = match t.get("layer_types").and_then(|v| v.as_array()) {
        Some(kinds) => kinds.iter().filter(|k| k.as_str().is_some_and(|s| s.contains("full_attention") || s == "attention")).count() as u64,
        None => match n("full_attention_interval") {
            Some(every) if every > 0 => layers / every,
            _ => layers,
        },
    };
    let heads = n("num_key_value_heads").or(n("num_attention_heads")).unwrap_or(0);
    let head_dim = n("head_dim").or_else(|| Some(n("hidden_size")? / n("num_attention_heads")?.max(1))).unwrap_or(0);
    let kv = attention_layers * 2 * heads * head_dim * 2;
    (max_context, (kv > 0).then_some(kv))
}

/// A lalamo `config.json` (uzu's converted folders): KV bytes per token over every attention layer
/// (bf16 keys and values) and the longest sequence its position encoding allows.
fn lalamo_kv(config: &serde_json::Value) -> (u64, Option<u32>) {
    fn walk(node: &serde_json::Value, total: &mut u64, longest: &mut Option<u32>) {
        match node {
            serde_json::Value::Object(map) => {
                if map.get("type").and_then(|v| v.as_str()) == Some("AttentionConfig") {
                    let groups = map.get("num_groups").and_then(|v| v.as_u64()).or(map.get("num_heads").and_then(|v| v.as_u64())).unwrap_or(0);
                    *total += 2 * groups * map.get("head_dim").and_then(|v| v.as_u64()).unwrap_or(0) * 2;
                }
                if let Some(n) = map.get("rope_config").and_then(|r| r.get("max_sequence_length")).and_then(|v| v.as_u64()) {
                    *longest = Some((*longest).unwrap_or(0).max(n as u32));
                }
                map.values().for_each(|v| walk(v, total, longest));
            }
            serde_json::Value::Array(items) => items.iter().for_each(|v| walk(v, total, longest)),
            _ => {}
        }
    }
    let (mut total, mut longest) = (0, None);
    walk(config, &mut total, &mut longest);
    (total, longest)
}

pub struct GgufFacts {
    pub context_length: Option<u32>,
    pub kv_bytes_per_token: Option<u64>,
}

/// The few numbers 8gent needs from a GGUF file's header: the trained context length and the shape of
/// the attention layers (f16 keys and values in every block).
pub fn gguf_facts(path: &Path) -> Option<GgufFacts> {
    let mut r = BufReader::with_capacity(1 << 16, std::fs::File::open(path).ok()?);
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic).ok()?;
    if &magic != b"GGUF" {
        return None;
    }
    let _version = read_u32(&mut r)?;
    let _tensors = read_u64(&mut r)?;
    let count = read_u64(&mut r)?;
    let mut wanted: BTreeMap<&str, u64> = BTreeMap::new();
    for _ in 0..count.min(100_000) {
        let key = read_string(&mut r)?;
        let kind = read_u32(&mut r)?;
        let suffix = ["context_length", "block_count", "embedding_length", "attention.head_count_kv", "attention.head_count"]
            .into_iter()
            .find(|s| key.ends_with(&format!(".{s}")) && key.matches('.').count() == s.matches('.').count() + 1);
        match (suffix, read_scalar(&mut r, kind)?) {
            (Some(s), Some(v)) => {
                wanted.insert(s, v);
            }
            _ => {}
        }
    }
    let get = |k: &str| wanted.get(k).copied();
    let kv = (|| {
        let head_dim = get("embedding_length")? / get("attention.head_count")?.max(1);
        Some(get("block_count")? * 2 * get("attention.head_count_kv").or(get("attention.head_count"))? * head_dim * 2)
    })();
    Some(GgufFacts { context_length: get("context_length").map(|v| v as u32), kv_bytes_per_token: kv.filter(|n| *n > 0) })
}

fn read_u32(r: &mut impl Read) -> Option<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b).ok()?;
    Some(u32::from_le_bytes(b))
}

fn read_u64(r: &mut impl Read) -> Option<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b).ok()?;
    Some(u64::from_le_bytes(b))
}

fn read_string(r: &mut impl Read) -> Option<String> {
    let len = read_u64(r)?;
    if len > 1 << 20 {
        return None;
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Reads one metadata value, returning it when it is an unsigned integer; everything else is skipped.
/// The outer None means the file ended or is malformed.
fn read_scalar<R: Read + Seek>(r: &mut R, kind: u32) -> Option<Option<u64>> {
    let fixed = |kind: u32| -> Option<i64> {
        Some(match kind {
            0 | 1 | 7 => 1,
            2 | 3 => 2,
            4..=6 => 4,
            10..=12 => 8,
            _ => return None,
        })
    };
    match kind {
        4 => read_u32(r).map(|v| Some(v as u64)),
        10 => read_u64(r).map(Some),
        8 => {
            let len = read_u64(r)?;
            r.seek_relative(len as i64).ok()?;
            Some(None)
        }
        9 => {
            let elem = read_u32(r)?;
            let n = read_u64(r)?;
            if elem == 8 {
                for _ in 0..n {
                    let len = read_u64(r)?;
                    r.seek_relative(len as i64).ok()?;
                }
            } else {
                r.seek_relative(fixed(elem)? * n as i64).ok()?;
            }
            Some(None)
        }
        k => {
            r.seek_relative(fixed(k)?).ok()?;
            Some(None)
        }
    }
}

// ------------------------------------------------------------------------------------------- helpers

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir() && !file_name(p).starts_with('.')).collect();
    out.sort();
    out
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn dir_size(dir: &Path, suffix: &str) -> u64 {
    std::fs::read_dir(dir).into_iter().flatten().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(suffix)).filter_map(|e| std::fs::metadata(e.path()).ok()).map(|m| m.len()).sum()
}

/// The lines of a `a|b|c|d` list, comments and blanks skipped.
fn conf_lines(conf: &Path) -> Vec<Vec<String>> {
    let Ok(text) = std::fs::read_to_string(conf) else { return vec![] };
    text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(|l| l.split('|').map(|f| f.trim().to_string()).collect()).collect()
}

/// The newest downloaded snapshot of a Hugging Face repo in the shared cache, if it is there.
fn hf_snapshot(repo: &str) -> Option<PathBuf> {
    let cache = std::env::var_os("HF_HUB_CACHE").map(PathBuf::from).unwrap_or(paths::user_home().join(".cache/huggingface/hub"));
    let snapshots = cache.join(format!("models--{}", repo.replace('/', "--"))).join("snapshots");
    subdirs(&snapshots).into_iter().max_by_key(|d| std::fs::metadata(d).and_then(|m| m.modified()).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_merge_formats() {
        for name in ["ahmedandaloes/Qwythos-9B-v2-MLX-4bit", "Qwythos-9B-v2-GGUF", "Qwythos-9B-v2"] {
            assert_eq!(model_id(name), "qwythos-9b-v2", "{name}");
        }
        assert_eq!(model_id("OmAr-Kader-DeV/OxCoder-9B-mlx-4Bit"), "oxcoder-9b");
        assert_eq!(model_id("unsloth/Qwen3.5-0.8B-GGUF"), "qwen3.5-0.8b");
        assert_eq!(model_id("OrionLLM_GRM-3.2-Cliff-GGUF"), "orionllm-grm-3.2-cliff");
        assert_eq!(model_id("llama3.2:3b"), "llama3.2-3b");
        assert_eq!(display_name("Qwythos-9B-v2-MLX-4bit"), "Qwythos-9B-v2");
    }

    #[test]
    fn quants() {
        assert_eq!(gguf_quant("Qwen3.5-0.8B-Q4_K_M.gguf").as_deref(), Some("Q4_K_M"));
        assert_eq!(gguf_quant("Qwythos-9B-v2-MTP-Q6_K.gguf").as_deref(), Some("Q6_K"));
        assert_eq!(gguf_quant("model.gguf"), None);
        assert_eq!(mlx_quant("OxCoder-9B-mlx-4Bit").as_deref(), Some("4-bit"));
    }

    #[test]
    fn library_merges_builds() {
        let mut lib = Library::default();
        let b = |format: &str, reference: &str| Build { format: format.into(), reference: reference.into(), ..Default::default() };
        lib.add("m", "M", b("gguf", "/a.gguf"));
        lib.add("m", "M", b("mlx", "org/m-mlx"));
        lib.add("m", "M", b("gguf", "/a.gguf"));
        assert_eq!(lib.models.len(), 1);
        assert_eq!(lib.models[0].builds.len(), 2);
    }

    #[test]
    fn qwen_config_counts_only_attention_layers() {
        let config = serde_json::json!({"text_config": {"max_position_embeddings": 262144, "num_hidden_layers": 32,
            "full_attention_interval": 4, "num_key_value_heads": 4, "num_attention_heads": 16, "head_dim": 256}});
        let (ctx, kv) = hf_config_facts(&config);
        assert_eq!(ctx, Some(262144));
        assert_eq!(kv, Some(8 * 2 * 4 * 256 * 2)); // 32 KB a token, as measured on the 9B
    }
}
