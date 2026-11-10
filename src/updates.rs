//! Checking for newer engines and installing them. Two ways so far, named in a loader's `[update]`:
//!
//! - `brew`: the engine is a Homebrew formula; Homebrew says whether it is outdated and upgrades it.
//! - `github-prebuilt`: versions come from one GitHub repository's tags, ready-made engines from
//!   another's releases. A download is checked against the SHA-256 GitHub publishes for it, must run
//!   and offer the options 8gent uses, and the engine it replaces is kept for going back.
//!
//! Attached loaders (`app`) are updated by their own apps. Downloads use the system's `curl` and
//! `tar`, which macOS, Linux and Windows 10 and later all ship.

use crate::manifest::{Manifest, compare_versions, platform};
use crate::paths::{self, Paths};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default, Serialize)]
pub struct UpdateState {
    pub loader: String,
    pub current: Option<String>,
    pub latest: Option<String>,
    /// "current", "available", "held" (a newer one exists but this computer does not meet what it
    /// needs), "not_installed", "elsewhere" (its own app updates it) or "unknown".
    pub status: String,
    pub note: String,
    pub checked_at: Option<u64>,
    /// An update of this loader is running.
    pub busy: bool,
    /// How the last update ended, in words.
    pub last_result: Option<String>,
}

/// Run a program to its end, with a time limit; its output (both streams) on success, the reason on failure.
pub fn run(program: &Path, args: &[&str], limit: Duration) -> Result<String, String> {
    let mut child = Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("{}: {e}", program.display()))?;
    // read both pipes while waiting, or a talkative program fills one and never ends
    let mut out = child.stdout.take().unwrap();
    let mut err = child.stderr.take().unwrap();
    let read_out = std::thread::spawn(move || { let mut s = String::new(); let _ = std::io::Read::read_to_string(&mut out, &mut s); s });
    let read_err = std::thread::spawn(move || { let mut s = String::new(); let _ = std::io::Read::read_to_string(&mut err, &mut s); s });
    let deadline = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{} did not finish within {} s", program.display(), limit.as_secs()));
            }
            Err(e) => return Err(e.to_string()),
        }
    };
    let text = format!("{}{}", read_out.join().unwrap_or_default(), read_err.join().unwrap_or_default());
    if status.success() { Ok(text) } else { Err(text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("it failed").trim().to_string()) }
}

fn tool(name: &str, places: &[&str]) -> Option<PathBuf> {
    let candidates: Vec<String> = places.iter().map(|s| s.to_string()).collect();
    paths::find_program(&candidates, Some(name), &Paths::from_env())
}

pub fn brew() -> Option<PathBuf> {
    tool("brew", &["/opt/homebrew/bin/brew", "/usr/local/bin/brew", "/home/linuxbrew/.linuxbrew/bin/brew"])
}

fn curl_json(url: &str) -> Result<serde_json::Value, String> {
    let curl = tool("curl", &["/usr/bin/curl"]).ok_or("curl was not found")?;
    let mut last = String::new();
    // GitHub's API answers "try later" now and then
    for wait in [0, 5, 15] {
        std::thread::sleep(Duration::from_secs(wait));
        match run(&curl, &["-fsSL", "--max-time", "30", "-H", "Accept: application/vnd.github+json", url], Duration::from_secs(40)) {
            Ok(text) => return serde_json::from_str(&text).map_err(|e| format!("{url}: {e}")),
            Err(e) => last = e,
        }
    }
    Err(format!("{url}: {last}"))
}

/// The installed and the newest version of a Homebrew formula, and whether Homebrew calls it
/// outdated (its own judgement: version schemes change, as llama.cpp's did from 8680 to 0.9.0).
fn brew_versions(formula: &str) -> Result<(Option<String>, Option<String>, bool), String> {
    let brew = brew().ok_or("Homebrew was not found")?;
    let text = run(&brew, &["info", "--json=v2", formula], Duration::from_secs(120))?;
    let doc: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let f = doc.get("formulae").and_then(|f| f.get(0)).ok_or_else(|| format!("Homebrew does not know {formula}"))?;
    let installed = f.get("installed").and_then(|i| i.as_array()).and_then(|a| a.last()).and_then(|i| i.get("version")).and_then(|v| v.as_str()).map(String::from);
    let latest = f.get("versions").and_then(|v| v.get("stable")).and_then(|v| v.as_str()).map(String::from);
    Ok((installed, latest, f.get("outdated").and_then(|o| o.as_bool()).unwrap_or(false)))
}

/// The highest version among a repository's tags.
fn newest_tag(repo: &str) -> Result<String, String> {
    let tags = curl_json(&format!("https://api.github.com/repos/{repo}/tags?per_page=30"))?;
    tags.as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t.get("name")?.as_str())
        .filter(|n| n.trim_start_matches('v').chars().next().is_some_and(|c| c.is_ascii_digit()))
        .max_by(|a, b| compare_versions(a, b))
        .map(|s| s.trim_start_matches('v').to_string())
        .ok_or_else(|| format!("{repo} has no version tags"))
}

/// The download address and published SHA-256 of a release file, looked for in every release of `repo`.
fn release_asset(repo: &str, name: &str) -> Result<Option<(String, String)>, String> {
    let releases = curl_json(&format!("https://api.github.com/repos/{repo}/releases?per_page=30"))?;
    for r in releases.as_array().into_iter().flatten() {
        for a in r.get("assets").and_then(|a| a.as_array()).into_iter().flatten() {
            if a.get("name").and_then(|n| n.as_str()) == Some(name) {
                let digest = a.get("digest").and_then(|d| d.as_str()).and_then(|d| d.strip_prefix("sha256:"));
                if let (Some(url), Some(sha)) = (a.get("browser_download_url").and_then(|u| u.as_str()), digest) {
                    return Ok(Some((url.to_string(), sha.to_string())));
                }
            }
        }
    }
    Ok(None)
}

/// One file of a release: its name, download address and the SHA-256 GitHub publishes for it.
#[derive(Clone, Debug, Serialize)]
pub struct ReleaseFile {
    pub name: String,
    pub url: String,
    pub sha256: Option<String>,
}

/// The newest published release of a repository: its version (the tag without a leading "v"), its
/// page, and its files.
pub fn latest_release(repo: &str) -> Result<(String, String, Vec<ReleaseFile>), String> {
    let r = curl_json(&format!("https://api.github.com/repos/{repo}/releases/latest"))?;
    let tag = r.get("tag_name").and_then(|t| t.as_str()).ok_or_else(|| format!("{repo} has no published release"))?;
    let page = r.get("html_url").and_then(|u| u.as_str()).unwrap_or_default().to_string();
    let files = r
        .get("assets")
        .and_then(|a| a.as_array())
        .into_iter()
        .flatten()
        .filter_map(|a| {
            Some(ReleaseFile {
                name: a.get("name")?.as_str()?.to_string(),
                url: a.get("browser_download_url")?.as_str()?.to_string(),
                sha256: a.get("digest").and_then(|d| d.as_str()).and_then(|d| d.strip_prefix("sha256:")).map(String::from),
            })
        })
        .collect();
    Ok((tag.trim_start_matches('v').to_string(), page, files))
}

/// Download a release file and check it against its published SHA-256. Without a published hash, or
/// with a different one, nothing is kept.
pub fn download_verified(file: &ReleaseFile, dest: &Path) -> Result<(), String> {
    let sha = file.sha256.as_deref().ok_or_else(|| format!("GitHub publishes no SHA-256 for {}, so the download cannot be checked.", file.name))?;
    let curl = tool("curl", &["/usr/bin/curl"]).ok_or("curl was not found")?;
    run(&curl, &["-fsSL", "--retry", "3", "--max-time", "1800", "-o", &dest.to_string_lossy(), &file.url], Duration::from_secs(1900))?;
    let got = sha256_file(dest)?;
    if got != sha.to_lowercase() {
        let _ = std::fs::remove_file(dest);
        return Err(format!("The download's SHA-256 is {got}; GitHub publishes {sha}. Nothing was installed."));
    }
    Ok(())
}

fn asset_name(m: &Manifest, version: &str) -> String {
    m.update.asset.replace("{version}", version).replace("{platform}", platform())
}

/// What stands between this computer and `version` of a loader: the first requirement that applies
/// to that version and is not met.
pub fn held_by(m: &Manifest, version: &str, os_version: &str) -> Option<String> {
    m.requires.iter().find_map(|r| {
        let applies = r.os == std::env::consts::OS && r.since.as_deref().is_none_or(|since| compare_versions(version, since) != std::cmp::Ordering::Less);
        let met = compare_versions(os_version, &r.min) != std::cmp::Ordering::Less;
        (applies && !met).then(|| format!("It waits for {} {}. {}", os_name(&r.os), r.min, r.note).trim().to_string())
    })
}

fn os_name(os: &str) -> &str {
    match os {
        "macos" => "macOS",
        "windows" => "Windows",
        "linux" => "Linux",
        other => other,
    }
}

/// Look for a newer engine. `installed` is the version detection found (None: not installed).
pub fn check(m: &Manifest, installed: Option<&str>, os_version: &str) -> UpdateState {
    let mut s = UpdateState { loader: m.id.clone(), current: installed.map(String::from), checked_at: Some(crate::stats::now()), ..Default::default() };
    let result: Result<(), String> = (|| {
        match m.update.method.as_str() {
            "brew" => {
                let (have, latest, outdated) = brew_versions(&m.update.source)?;
                s.latest = latest;
                if have.is_none() {
                    s.status = "not_installed".into();
                    s.note = format!("Homebrew can install it: brew install {}", m.update.source);
                } else {
                    s.current = have;
                    s.status = if outdated { "available" } else { "current" }.into();
                }
            }
            "github-prebuilt" => {
                let latest = newest_tag(&m.update.upstream)?;
                s.latest = Some(latest.clone());
                let newer = installed.is_none_or(|have| compare_versions(&latest, have) == std::cmp::Ordering::Greater);
                if !newer {
                    s.status = "current".into();
                } else if let Some(reason) = held_by(m, &latest, os_version) {
                    s.status = "held".into();
                    s.note = reason;
                } else if release_asset(&m.update.source, &asset_name(m, &latest))?.is_some() {
                    s.status = if installed.is_some() { "available" } else { "not_installed" }.into();
                } else {
                    s.status = "held".into();
                    s.note = format!("No ready-made engine for {} {latest} on this kind of computer has been published yet.", m.name);
                }
            }
            "app" => {
                s.status = "elsewhere".into();
                s.note = format!("{} updates itself.", m.name);
            }
            _ => s.status = "unknown".into(),
        }
        Ok(())
    })();
    if let Err(e) = result {
        s.status = "unknown".into();
        s.note = format!("The check did not work: {e}");
    }
    s
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(|e| e.to_string())?;
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Where a loader's own engine lives once 8gent has installed it.
pub fn install_dir(m: &Manifest, paths: &Paths) -> PathBuf {
    PathBuf::from(paths::expand(&m.update.install_dir, paths))
}

/// Install the newest engine. Returns the version now installed. The caller has stopped the engine.
pub fn apply(m: &Manifest, paths: &Paths, os_version: &str) -> Result<String, String> {
    match m.update.method.as_str() {
        "brew" => {
            let brew = brew().ok_or("Homebrew was not found")?;
            let (have, _, _) = brew_versions(&m.update.source)?;
            let verb = if have.is_some() { "upgrade" } else { "install" };
            run(&brew, &[verb, &m.update.source], Duration::from_secs(30 * 60))?;
            let (now, _, _) = brew_versions(&m.update.source)?;
            now.ok_or_else(|| format!("Homebrew finished but {} is not installed", m.update.source))
        }
        "github-prebuilt" => {
            let version = newest_tag(&m.update.upstream)?;
            if let Some(reason) = held_by(m, &version, os_version) {
                return Err(reason);
            }
            let name = asset_name(m, &version);
            let (url, sha) = release_asset(&m.update.source, &name)?.ok_or_else(|| format!("No ready-made {name} has been published."))?;
            let stage = paths.home.join("engines").join(".staging").join(format!("{}-{version}", m.id));
            let _ = std::fs::remove_dir_all(&stage);
            std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
            let archive = stage.join(&name);
            let curl = tool("curl", &["/usr/bin/curl"]).ok_or("curl was not found")?;
            run(&curl, &["-fsSL", "--retry", "3", "--max-time", "1800", "-o", &archive.to_string_lossy(), &url], Duration::from_secs(1900))?;
            let got = sha256_file(&archive)?;
            if got != sha.to_lowercase() {
                let _ = std::fs::remove_dir_all(&stage);
                return Err(format!("The download's SHA-256 is {got}; GitHub publishes {sha}. Nothing was installed."));
            }
            let tar = tool("tar", &["/usr/bin/tar"]).ok_or("tar was not found")?;
            let unpacked = stage.join("unpacked");
            std::fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
            run(&tar, &["-xzf", &archive.to_string_lossy(), "-C", &unpacked.to_string_lossy()], Duration::from_secs(300))?;
            let bin_name = m.update.binary.clone();
            let fresh = unpacked.join(&bin_name);
            if !fresh.is_file() {
                return Err(format!("{name} holds no {bin_name}. Nothing was installed."));
            }
            // it must run here and offer what 8gent's start line uses
            if !m.update.verify_args.is_empty() {
                let args: Vec<&str> = m.update.verify_args.iter().map(String::as_str).collect();
                let help = run(&fresh, &args, Duration::from_secs(60)).map_err(|e| format!("The downloaded engine does not run here: {e}"))?;
                if let Some(missing) = m.update.verify_contains.iter().find(|needle| !help.contains(needle.as_str())) {
                    return Err(format!("The downloaded engine lacks {missing}. Nothing was installed."));
                }
            }
            let dir = install_dir(m, paths);
            let previous = dir.join("previous");
            std::fs::create_dir_all(&previous).map_err(|e| e.to_string())?;
            for f in [bin_name.as_str(), "VERSION"] {
                if dir.join(f).is_file() {
                    std::fs::rename(dir.join(f), previous.join(f)).map_err(|e| e.to_string())?;
                }
            }
            std::fs::copy(&fresh, dir.join(&bin_name)).map_err(|e| e.to_string())?;
            let stamp = std::fs::read_to_string(unpacked.join("VERSION")).map(|v| v.trim().to_string()).unwrap_or(version.clone());
            std::fs::write(dir.join("VERSION"), format!("{stamp}\n")).map_err(|e| e.to_string())?;
            let _ = std::fs::remove_dir_all(&stage);
            Ok(stamp)
        }
        other => Err(format!("8gent does not update loaders of kind \"{other}\".")),
    }
}

/// Put back the engine that `apply` replaced. True if there was one.
pub fn roll_back(m: &Manifest, paths: &Paths) -> bool {
    if m.update.method != "github-prebuilt" {
        return false;
    }
    let dir = install_dir(m, paths);
    let previous = dir.join("previous");
    if !previous.join(&m.update.binary).is_file() {
        return false;
    }
    let mut ok = true;
    for f in [m.update.binary.as_str(), "VERSION"] {
        if previous.join(f).is_file() {
            ok &= std::fs::rename(previous.join(f), dir.join(f)).is_ok();
        }
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uzu() -> Manifest {
        crate::manifest::load_all(&Paths::at(std::env::temp_dir().join("8gent-none"))).unwrap().into_iter().find(|m| m.id == "uzu").unwrap()
    }

    #[test]
    fn a_version_that_needs_a_newer_os_is_held() {
        if std::env::consts::OS != "macos" {
            return;
        }
        let m = uzu();
        assert!(held_by(&m, "0.6.17", "26.2").unwrap().starts_with("It waits for macOS 26.4."));
        assert!(held_by(&m, "0.6.18", "26.3").is_some());
        assert!(held_by(&m, "0.6.17", "26.4").is_none());
        assert!(held_by(&m, "0.6.3", "26.2").is_none(), "an older engine needs nothing newer");
    }

    #[test]
    fn asset_names_and_hashes() {
        assert_eq!(asset_name(&uzu(), "0.6.17"), format!("uzu-engine-0.6.17-{}.tar.gz", platform()));
        let file = std::env::temp_dir().join(format!("8gent-sha-{}", std::process::id()));
        std::fs::write(&file, b"abc").unwrap();
        assert_eq!(sha256_file(&file).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn run_gives_output_or_the_reason() {
        if cfg!(windows) {
            return;
        }
        assert_eq!(run(Path::new("/bin/echo"), &["hello"], Duration::from_secs(5)).unwrap().trim(), "hello");
        assert!(run(Path::new("/bin/sh"), &["-c", "echo no such thing >&2; exit 3"], Duration::from_secs(5)).unwrap_err().contains("no such thing"));
        assert!(run(Path::new("/bin/sleep"), &["5"], Duration::from_millis(300)).unwrap_err().contains("did not finish"));
    }
}
