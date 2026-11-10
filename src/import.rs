//! Taking over from uzu Home and TensorFold Home: on 8gent's first run, select what the Home app had
//! selected, with its context window, so the switch needs no setting up.

use crate::config::{Config, Selection};
use crate::library::Library;
use crate::paths;
use std::collections::BTreeMap;
use std::path::Path;

fn env_file(path: &Path) -> BTreeMap<String, String> {
    let Ok(text) = std::fs::read_to_string(path) else { return BTreeMap::new() };
    text.lines().filter(|l| !l.trim_start().starts_with('#')).filter_map(|l| l.split_once('=')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect()
}

/// The model list line `key|label|kind|reference` for `key`.
fn conf_reference(conf: &Path, key: &str) -> Option<(String, String)> {
    let text = std::fs::read_to_string(conf).ok()?;
    text.lines().filter(|l| !l.trim_start().starts_with('#')).find_map(|l| {
        let f: Vec<&str> = l.split('|').map(str::trim).collect();
        (f.len() == 4 && f[0] == key).then(|| (f[2].to_string(), f[3].to_string()))
    })
}

/// uzu Home's selection if it is installed, else TensorFold Home's. uzu's number of conversations is
/// carried into the config too.
pub fn selection_from_home_apps(library: &Library, cfg: &mut Config) -> Option<Selection> {
    let support = paths::user_home().join("Library/Application Support");
    let model_with = |format: &str, reference: &str| library.models.iter().find(|m| m.builds.iter().any(|b| b.format == format && b.reference == reference)).map(|m| m.id.clone());

    let uzu = support.join("UzuHome/backend");
    let env = env_file(&uzu.join("config.env"));
    if let Some(key) = env.get("MODEL") {
        if let Some((kind, reference)) = conf_reference(&uzu.join("models.conf"), key) {
            let format = if kind == "local" { "uzu" } else { "mirai" };
            if let Some(model) = model_with(format, &reference) {
                if let Some(n) = env.get("SESSIONS").and_then(|v| v.parse::<u32>().ok()) {
                    cfg.loaders.entry("uzu".into()).or_default().sessions = Some(n);
                }
                return Some(Selection { loader: "uzu".into(), model, context: env.get("CONTEXT").and_then(|v| v.parse().ok()).unwrap_or(0), thinking: env.get("THINKING").is_some_and(|v| v == "on") });
            }
        }
    }

    let tf = support.join("TensorFoldLauncher");
    let env = env_file(&tf.join("config.env"));
    let key = env.get("MODEL")?;
    let (_, reference) = conf_reference(&tf.join("models.conf"), key)?;
    let model = model_with("mlx", &reference)?;
    Some(Selection { loader: "tensorfold".into(), model, context: env.get("CONTEXT").and_then(|v| v.parse().ok()).unwrap_or(0), thinking: env.get("THINKING").is_some_and(|v| v == "on") })
}
