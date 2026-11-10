//! Starting, watching and stopping the one engine that is loaded. A managed loader's engine is a
//! child process of the service; an attached loader's engine is another app's server, only checked.

use crate::config::{Config, DEFAULT_CONTEXT, Selection};
use crate::library::{Build, Library};
use crate::manifest::{Kind, Manifest, Vars, first_version};
use crate::paths::{self, Paths};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::process::{Child, Command};

/// What detection found for a loader on this computer.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Detected {
    pub program: Option<String>,
    pub version: Option<String>,
    /// An attached server answered when last asked.
    pub reachable: Option<bool>,
}

/// Everything needed to start (or attach to) an engine for one selection.
#[derive(Clone, Debug)]
pub struct Plan {
    pub loader: String,
    pub loader_name: String,
    pub model: String,
    pub build: Build,
    pub context: u32,
    pub thinking: bool,
    pub sessions: u32,
    pub managed: bool,
    pub program: Option<PathBuf>,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    /// Filled in at start for a managed engine (the port is chosen then).
    pub base_url: String,
    pub ready_path: String,
    pub ready_timeout: Duration,
    /// The start line depends on the thinking mode, so changing it means a restart.
    pub thinking_in_start: bool,
    /// The model name the engine expects, or None to ask the engine.
    pub engine_model: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Running {
    pub loader: String,
    pub model: String,
    pub context: u32,
    pub thinking: bool,
    pub sessions: u32,
    pub managed: bool,
    pub base_url: String,
    pub engine_model: String,
    pub pid: Option<u32>,
    pub loader_version: Option<String>,
    pub started_at: u64,
    pub load_seconds: f64,
    #[serde(skip)]
    pub thinking_in_start: bool,
    #[serde(skip)]
    pub build: Build,
}

impl Running {
    /// Whether this engine can answer for `plan` as it is, without a restart.
    pub fn serves(&self, plan: &Plan) -> bool {
        self.loader == plan.loader
            && self.model == plan.model
            && self.build.reference == plan.build.reference
            && (!self.managed || (self.context == plan.context && self.sessions == plan.sessions))
            && (!self.thinking_in_start || self.thinking == plan.thinking)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Stopped,
    Loading,
    Ready,
    Error,
}

struct Inner {
    child: Option<Child>,
    running: Option<Running>,
    state: State,
    last_error: Option<String>,
    loading: Option<(String, String, Instant)>,
}

pub struct Engine {
    paths: Paths,
    inner: Mutex<Inner>,
    http: reqwest::Client,
}

/// The context a selection gets when it does not name one: 65,536, or the model's own maximum when
/// that is smaller.
pub fn default_context(build: &Build) -> u32 {
    build.max_context.map_or(DEFAULT_CONTEXT, |max| max.min(DEFAULT_CONTEXT))
}

/// The build of `model` that `manifest` can run.
pub fn build_for<'a>(library: &'a Library, model: &str, manifest: &Manifest) -> Option<&'a Build> {
    let m = library.get(model)?;
    m.builds.iter().find(|b| b.loader.as_deref() == Some(manifest.id.as_str())).or_else(|| m.builds.iter().find(|b| b.loader.is_none() && manifest.runs(&b.format)))
}

/// Where a loader's program is: the config's override, then the loader definition's places.
pub fn find_loader_program(manifest: &Manifest, cfg: &Config, paths: &Paths) -> Option<PathBuf> {
    if let Some(p) = cfg.loader(&manifest.id).program {
        let p = PathBuf::from(paths::expand(&p, paths));
        return p.is_file().then_some(p);
    }
    paths::find_program(&manifest.detect.paths, manifest.detect.command.as_deref(), paths)
}

/// Turn a selection into a start plan, or say in plain words why it cannot run here.
pub fn plan(sel: &Selection, manifests: &[Manifest], library: &Library, cfg: &Config, paths: &Paths, os_version: &str) -> Result<Plan, String> {
    let manifest = manifests.iter().find(|m| m.id == sel.loader).ok_or_else(|| format!("There is no loader called \"{}\".", sel.loader))?;
    if !manifest.supports_platform() {
        return Err(format!("{} does not run on this kind of computer ({}).", manifest.name, crate::manifest::platform()));
    }
    let model = library.get(&sel.model).ok_or_else(|| format!("There is no model called \"{}\" in the library.", sel.model))?;
    let build = build_for(library, &sel.model, manifest).ok_or_else(|| {
        if manifest.kind == Kind::Attach {
            return format!("{} only serves the models it has itself, and {} is not one of them (or its server is not running).", manifest.name, model.name);
        }
        let formats: Vec<&str> = model.builds.iter().map(|b| b.format.as_str()).collect();
        format!("{} cannot run {}: it is here as {}, and {} runs {}.", manifest.name, model.name, formats.join(", "), manifest.name, manifest.formats.join(", "))
    })?;
    let settings = cfg.loader(&manifest.id);
    // the window asked for, else the one last chosen for this model, else the default
    let remembered = cfg.contexts.get(&sel.model).copied().filter(|c| build.max_context.is_none_or(|max| *c <= max));
    let context = if sel.context > 0 { sel.context } else { remembered.unwrap_or_else(|| default_context(build)) };
    if let Some(max) = build.max_context {
        if context > max {
            return Err(format!("{} was built for at most {max} tokens of context; {context} was asked for.", model.name));
        }
    }
    let sessions = settings.sessions.unwrap_or(if manifest.capabilities.sessions > 0 { 1 } else { 0 });
    let mut vars = Vars { model: build.reference.clone(), model_id: sel.model.clone(), port: 0, context, sessions, thinking: sel.thinking, os_version: os_version.to_string() };

    if manifest.kind == Kind::Attach {
        let attach = manifest.attach.as_ref().ok_or_else(|| format!("{}'s definition has no [attach] section.", manifest.name))?;
        let base = settings.base_url.unwrap_or(attach.base_url.clone());
        let base = base.trim_end_matches('/').trim_end_matches("/v1").to_string();
        return Ok(Plan {
            loader: manifest.id.clone(), loader_name: manifest.name.clone(), model: sel.model.clone(), build: build.clone(), context, thinking: sel.thinking, sessions,
            managed: false, program: None, args: vec![], env: BTreeMap::new(), base_url: base, ready_path: "/v1/models".into(),
            ready_timeout: Duration::from_secs(5), thinking_in_start: false, engine_model: Some(vars.fill(&manifest.api.model_id, paths)),
        });
    }

    let start = manifest.start.as_ref().ok_or_else(|| format!("{}'s definition has no [start] section.", manifest.name))?;
    let variant = match &build.variant {
        Some(v) => Some(manifest.variants.get(v).ok_or_else(|| format!("{} has no variant called \"{v}\".", manifest.name))?),
        None => None,
    };
    let program = match variant.filter(|v| !v.program.is_empty()) {
        Some(v) => paths::find_program(&v.program, None, paths).ok_or_else(|| format!("{} is not installed (looked in {}).", if v.name.is_empty() { &manifest.name } else { &v.name }, v.program.join(", ")))?,
        None => find_loader_program(manifest, cfg, paths).ok_or_else(|| format!("{} is not installed on this computer.", manifest.name))?,
    };
    // the port is filled in when the engine starts; keep the placeholder until then
    vars.port = 0;
    let mut args: Vec<String> = variant.map(|v| v.args_prefix.iter().map(|a| paths::expand(a, paths)).collect()).unwrap_or_default();
    args.extend(start.args.iter().cloned());
    for w in &start.when {
        if vars.holds(&w.condition) {
            args.extend(w.args.iter().cloned());
        }
    }
    args.extend(settings.extra_args.iter().cloned());
    let mut env = start.env.clone();
    for w in &start.env_when {
        if vars.holds(&w.condition) {
            env.extend(w.set.clone());
        }
    }
    env.extend(settings.env.clone());
    let thinking_in_start = start.when.iter().any(|w| w.condition.contains("thinking"));
    let engine_model = (manifest.api.model_id != "{auto}").then(|| vars.fill(&manifest.api.model_id, paths));
    // "{port}" stays in the arguments; every other value is filled in now
    let args = args.iter().map(|a| a.split("{port}").map(|part| vars.fill(part, paths)).collect::<Vec<_>>().join("{port}")).collect();
    Ok(Plan {
        loader: manifest.id.clone(), loader_name: manifest.name.clone(), model: sel.model.clone(), build: build.clone(), context, thinking: sel.thinking, sessions,
        managed: true, program: Some(program), args, env, base_url: String::new(), ready_path: start.ready_path.clone(),
        ready_timeout: Duration::from_secs(start.ready_timeout_s), thinking_in_start, engine_model,
    })
}

/// The program and version of every loader on this computer. Version commands are run with a short
/// time limit; a loader without one reports no version.
pub fn detect(manifests: &[Manifest], cfg: &Config, paths: &Paths) -> BTreeMap<String, Detected> {
    let mut out = BTreeMap::new();
    for m in manifests {
        let program = find_loader_program(m, cfg, paths);
        let mut version = None;
        if let Some(p) = &program {
            if let Some(file) = &m.detect.version_file {
                version = p.parent().and_then(|d| std::fs::read_to_string(d.join(file)).ok()).map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
            }
            if version.is_none() && !m.detect.version_args.is_empty() {
                version = run_for_version(p, &m.detect.version_args);
            }
        }
        out.insert(m.id.clone(), Detected { program: program.map(|p| p.to_string_lossy().into_owned()), version, reachable: None });
    }
    out
}

fn run_for_version(program: &PathBuf, args: &[String]) -> Option<String> {
    let mut child = std::process::Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().ok()?;
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(40)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let out = child.wait_with_output().ok()?;
    let text = format!("{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    // a line that says "version" is the one meant; other lines can hold unrelated numbers
    let line = text.lines().find(|l| l.to_lowercase().contains("version")).unwrap_or_else(|| text.lines().find(|l| !l.trim().is_empty()).unwrap_or(""));
    first_version(line)
}

fn free_port() -> Result<u16, String> {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|e| format!("no free port: {e}"))?;
    Ok(l.local_addr().map_err(|e| e.to_string())?.port())
}

impl Engine {
    pub fn new(paths: Paths, http: reqwest::Client) -> Self {
        Self { paths, http, inner: Mutex::new(Inner { child: None, running: None, state: State::Stopped, last_error: None, loading: None }) }
    }

    pub fn state(&self) -> State {
        self.inner.lock().unwrap().state
    }

    pub fn last_error(&self) -> Option<String> {
        self.inner.lock().unwrap().last_error.clone()
    }

    /// What is being loaded right now: loader, model, seconds so far.
    pub fn loading(&self) -> Option<(String, String, f64)> {
        self.inner.lock().unwrap().loading.as_ref().map(|(l, m, t)| (l.clone(), m.clone(), t.elapsed().as_secs_f64()))
    }

    /// The running engine, after checking that a managed one has not exited behind our back.
    pub fn current(&self) -> Option<Running> {
        let mut g = self.inner.lock().unwrap();
        // while an engine is loading, the start itself watches for its exit and reports why
        if g.loading.is_some() {
            return g.running.clone();
        }
        if let Some(child) = g.child.as_mut() {
            if let Ok(Some(status)) = child.try_wait() {
                let loader = g.running.as_ref().map(|r| r.loader.clone()).unwrap_or_default();
                g.child = None;
                g.running = None;
                g.state = State::Error;
                g.last_error = Some(format!("The engine stopped by itself ({status}). {}", log_tail(&self.paths.engine_log(&loader), 6)));
                let _ = std::fs::remove_file(self.paths.engine_record());
            }
        }
        g.running.clone()
    }

    /// Start the engine for `plan` and wait until it answers. The caller has stopped any other engine.
    pub async fn start(&self, mut plan: Plan, loader_version: Option<String>) -> Result<Running, String> {
        let began = Instant::now();
        {
            let mut g = self.inner.lock().unwrap();
            g.state = State::Loading;
            g.last_error = None;
            g.loading = Some((plan.loader.clone(), plan.model.clone(), began));
        }
        let result = self.start_inner(&mut plan, loader_version, began).await;
        let mut g = self.inner.lock().unwrap();
        g.loading = None;
        match &result {
            Ok(run) => {
                g.running = Some(run.clone());
                g.state = State::Ready;
            }
            Err(e) => {
                g.state = State::Error;
                g.last_error = Some(e.clone());
            }
        }
        result
    }

    async fn start_inner(&self, plan: &mut Plan, loader_version: Option<String>, began: Instant) -> Result<Running, String> {
        let mut pid = None;
        if plan.managed {
            let program = plan.program.clone().ok_or("no program")?;
            let port = free_port()?;
            plan.base_url = format!("http://127.0.0.1:{port}");
            let args: Vec<String> = plan.args.iter().map(|a| a.replace("{port}", &port.to_string())).collect();
            let log_path = self.paths.engine_log(&plan.loader);
            trim_log(&log_path, 4 << 20);
            let mut log = std::fs::OpenOptions::new().create(true).append(true).open(&log_path).map_err(|e| format!("{}: {e}", log_path.display()))?;
            {
                use std::io::Write;
                let _ = writeln!(log, "8gent: {} starting {} ({}) on 127.0.0.1:{port}, context {}: {} {}", crate::stats::now(), plan.model, plan.loader, plan.context, program.display(), args.join(" "));
            }
            let mut cmd = Command::new(&program);
            cmd.args(&args).envs(&plan.env).stdin(Stdio::null()).stdout(log.try_clone().map_err(|e| e.to_string())?).stderr(log).kill_on_drop(true);
            #[cfg(unix)]
            cmd.process_group(0);
            let child = cmd.spawn().map_err(|e| format!("{} could not be started ({}): {e}", plan.loader_name, program.display()))?;
            pid = child.id();
            let record = serde_json::json!({"pid": pid, "loader": plan.loader, "program": program.to_string_lossy(), "port": port});
            let _ = std::fs::write(self.paths.engine_record(), record.to_string());
            self.inner.lock().unwrap().child = Some(child);
        }

        if let Err(e) = self.wait_ready(plan).await {
            if plan.managed {
                self.stop().await;
            }
            return Err(e);
        }
        let engine_model = match plan.engine_model.clone() {
            Some(m) => m,
            None => self.first_model(&plan.base_url, &plan.model).await.unwrap_or(plan.model.clone()),
        };
        Ok(Running {
            loader: plan.loader.clone(), model: plan.model.clone(), context: plan.context, thinking: plan.thinking, sessions: plan.sessions, managed: plan.managed,
            base_url: plan.base_url.clone(), engine_model, pid, loader_version, started_at: crate::stats::now(), load_seconds: (began.elapsed().as_secs_f64() * 10.0).round() / 10.0,
            thinking_in_start: plan.thinking_in_start, build: plan.build.clone(),
        })
    }

    async fn wait_ready(&self, plan: &Plan) -> Result<(), String> {
        let url = format!("{}{}", plan.base_url, plan.ready_path);
        let deadline = Instant::now() + plan.ready_timeout;
        loop {
            if let Ok(resp) = self.http.get(&url).timeout(Duration::from_secs(3)).send().await {
                if resp.status().is_success() {
                    return Ok(());
                }
            }
            if plan.managed {
                let mut g = self.inner.lock().unwrap();
                let exited = match g.child.as_mut() {
                    Some(child) => child.try_wait().ok().flatten().map(|status| status.to_string()),
                    None => Some("it was stopped".to_string()),
                };
                if let Some(status) = exited {
                    g.child = None;
                    return Err(format!("{} stopped while loading {} ({status}). {}", plan.loader_name, plan.model, log_tail(&self.paths.engine_log(&plan.loader), 8)));
                }
            } else {
                return Err(format!("{}'s server is not answering at {}. Start it, then try again.", plan.loader_name, plan.base_url));
            }
            if Instant::now() >= deadline {
                return Err(format!("{} did not finish loading {} within {} s.", plan.loader_name, plan.model, plan.ready_timeout.as_secs()));
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }

    /// The model name the engine itself reports: ours if it lists it, else its first.
    async fn first_model(&self, base_url: &str, ours: &str) -> Option<String> {
        let body: serde_json::Value = self.http.get(format!("{base_url}/v1/models")).timeout(Duration::from_secs(5)).send().await.ok()?.json().await.ok()?;
        let ids: Vec<&str> = body.get("data")?.as_array()?.iter().filter_map(|m| m.get("id")?.as_str()).collect();
        ids.iter().find(|id| **id == ours).or(ids.first()).map(|s| s.to_string())
    }

    /// Stop the engine: ask it to end, and end it after 5 s if it has not. An attached server is left alone.
    pub async fn stop(&self) {
        let child = {
            let mut g = self.inner.lock().unwrap();
            g.running = None;
            g.loading = None;
            if g.state != State::Error {
                g.state = State::Stopped;
            }
            g.child.take()
        };
        if let Some(mut child) = child {
            terminate(&mut child).await;
        }
        let _ = std::fs::remove_file(self.paths.engine_record());
    }

    pub fn mark_stopped(&self) {
        let mut g = self.inner.lock().unwrap();
        if g.running.is_none() && g.child.is_none() {
            g.state = State::Stopped;
            g.last_error = None;
        }
    }

    /// An engine a previous run of the service left behind (it was killed before it could stop it)
    /// still holds the model's memory. End it, after checking the process really is that program.
    pub fn end_leftover(&self) {
        let Ok(text) = std::fs::read_to_string(self.paths.engine_record()) else { return };
        let _ = std::fs::remove_file(self.paths.engine_record());
        let Ok(rec) = serde_json::from_str::<serde_json::Value>(&text) else { return };
        let (Some(pid), Some(program)) = (rec.get("pid").and_then(|p| p.as_u64()), rec.get("program").and_then(|p| p.as_str())) else { return };
        #[cfg(unix)]
        {
            let out = std::process::Command::new("ps").args(["-p", &pid.to_string(), "-o", "command="]).output();
            let command = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
            if !command.trim().is_empty() && command.contains(program) {
                eprintln!("8gent: ending the engine an earlier run left behind (pid {pid})");
                unsafe {
                    libc::killpg(pid as i32, libc::SIGTERM);
                }
            }
        }
        #[cfg(not(unix))]
        let _ = (pid, program);
    }
}

async fn terminate(child: &mut Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        // the engine leads its own process group, so helpers it started end with it
        unsafe {
            libc::killpg(pid as i32, libc::SIGTERM);
        }
        if tokio::time::timeout(Duration::from_secs(5), child.wait()).await.is_ok() {
            return;
        }
        unsafe {
            libc::killpg(pid as i32, libc::SIGKILL);
        }
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}

/// The physical memory a process holds, in bytes (resident size; None where it cannot be read).
pub fn process_memory(pid: u32) -> Option<u64> {
    // macOS: the physical footprint, which counts the GPU memory an engine holds (what Activity
    // Monitor shows); the resident size leaves most of a loaded model out.
    #[cfg(target_os = "macos")]
    {
        let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
        let ok = unsafe { libc::proc_pid_rusage(pid as i32, libc::RUSAGE_INFO_V2, &mut info as *mut _ as *mut libc::rusage_info_t) };
        if ok == 0 {
            return Some(info.ri_phys_footprint);
        }
    }
    #[cfg(unix)]
    {
        let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &pid.to_string()]).output().ok()?;
        let kb: u64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
        Some(kb * 1024)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        None
    }
}

/// The last lines of a log as one sentence-like string, for error messages.
pub fn log_tail(path: &PathBuf, lines: usize) -> String {
    let Ok(text) = std::fs::read_to_string(path) else { return String::new() };
    let tail: Vec<&str> = text.lines().rev().filter(|l| !l.trim().is_empty() && !l.starts_with("8gent:")).take(lines).collect();
    if tail.is_empty() {
        return String::new();
    }
    format!("Its last output: {}", tail.into_iter().rev().collect::<Vec<_>>().join(" | "))
}

/// Keep the newest half of a log that has grown past `max` bytes.
fn trim_log(path: &PathBuf, max: u64) {
    let Ok(meta) = std::fs::metadata(path) else { return };
    if meta.len() <= max {
        return;
    }
    if let Ok(bytes) = std::fs::read(path) {
        let keep = &bytes[bytes.len() - (max as usize / 2)..];
        let start = keep.iter().position(|b| *b == b'\n').map_or(0, |i| i + 1);
        let _ = std::fs::write(path, &keep[start..]);
    }
}
