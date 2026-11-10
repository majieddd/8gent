//! The service: one OpenAI-compatible endpoint in front of whichever loader is selected, and the
//! control API (`/8gent/v1/...`) that the app, the CLI and kr8 use to see and change what runs.

use crate::config::{Config, MemoryMode, Selection};
use crate::engine::{self, Detected, Engine, Plan, Running, State as EngineState};
use crate::library::{self, Build, Library};
use crate::manifest::{self, Kind, Manifest, compare_versions};
use crate::normalize::{self, error_body};
use crate::paths::Paths;
use crate::stats::{Recorder, now};
use crate::{host, proxy};
use axum::body::Bytes;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Query, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

/// What a loaded engine takes on top of its weights and KV cache (measured on 9B models: 0.6 to 1.1 GB).
const RUNTIME_BYTES: u64 = 1_100_000_000;

pub struct Options {
    pub paths: Paths,
    /// Read the model lists of LM Studio and the earlier Home apps, and on the first run take over
    /// what uzu Home or TensorFold Home had selected. Tests switch it off.
    pub import_external: bool,
    /// How often the service looks for an idle or dead engine.
    pub housekeeping_every: Duration,
    /// Finish looking for loaders and models before `build` returns. Off for the real service, which
    /// must answer at once and looks in the background; tests switch it on.
    pub wait_for_scan: bool,
}

impl Options {
    pub fn new(paths: Paths) -> Self {
        Self { paths, import_external: true, housekeeping_every: Duration::from_secs(5), wait_for_scan: false }
    }
}

pub struct App {
    pub paths: Paths,
    pub cfg: RwLock<Config>,
    pub manifests: Vec<Manifest>,
    pub detected: RwLock<BTreeMap<String, Detected>>,
    /// What each place on disk held when it was last read, by source.
    parts: RwLock<BTreeMap<String, Library>>,
    /// What the attached servers listed when last asked: model id, name, build.
    attached: RwLock<Vec<(String, String, Build)>>,
    /// Places still being read (a folder macOS is asking permission for stays here until answered).
    pending: RwLock<std::collections::BTreeSet<String>>,
    scan_begun: std::sync::atomic::AtomicBool,
    first_run: std::sync::atomic::AtomicBool,
    pub library: RwLock<Library>,
    pub engine: Engine,
    /// Requests hold it shared while an engine answers them; starting, stopping or switching the
    /// engine takes it alone, so no request loses its engine half way.
    pub gate: Arc<tokio::sync::RwLock<()>>,
    pub stats: Recorder,
    pub http: reqwest::Client,
    pub os_version: String,
    pub total_memory: Option<u64>,
    /// Other apps on this computer that have a model loaded right now (the earlier Home apps).
    also_loaded: RwLock<Vec<String>>,
    /// Told to stop through the control API.
    pub quit: tokio::sync::Notify,
    /// The extra ports that are answering right now.
    pub aliases: RwLock<Vec<u16>>,
    /// Pieces of answers written since the service started; the monitor reads its rate.
    pub live_tokens: AtomicU64,
    /// One sample a second for the Monitor screen, the last three minutes.
    monitor: std::sync::Mutex<std::collections::VecDeque<Sample>>,
    last_activity: AtomicU64,
    started: Instant,
    import_external: bool,
    housekeeping_every: Duration,
}

/// What the service was doing during one second.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Sample {
    pub at: u64,
    pub tokens_per_second: f64,
    pub in_flight: u64,
    pub memory_bytes: Option<u64>,
    pub state: EngineState,
}

pub struct ApiErr {
    pub status: StatusCode,
    pub body: Value,
}

impl ApiErr {
    pub fn new(status: StatusCode, message: impl Into<String>, kind: &str, code: Option<&str>) -> Self {
        Self { status, body: error_body(message, kind, code) }
    }
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message, "invalid_request_error", None)
    }
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, message, "service_unavailable", Some("engine_unavailable"))
    }
}

impl IntoResponse for ApiErr {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

/// What a request's `model` field resolved to.
pub struct Target {
    pub selection: Selection,
    /// The name the client used, written back into the answer.
    pub requested: String,
}

impl App {
    /// Ready to serve at once: nothing here reads model folders or runs other programs. Looking for
    /// loaders and models happens in [`App::rescan`], in the background unless `wait_for_scan` is set.
    pub async fn build(opts: Options) -> Result<Arc<Self>, String> {
        let paths = opts.paths;
        paths.ensure().map_err(|e| format!("{}: {e}", paths.home.display()))?;
        let first_run = !paths.config().exists();
        let mut cfg = Config::load(&paths)?;
        if first_run {
            if opts.import_external {
                crate::import::settings_from_home_apps(&mut cfg);
            }
            cfg.save(&paths)?;
        }
        let manifests = manifest::load_all(&paths)?;
        let http = reqwest::Client::builder().connect_timeout(Duration::from_secs(5)).pool_idle_timeout(Duration::from_secs(30)).build().map_err(|e| e.to_string())?;
        let engine = Engine::new(paths.clone(), http.clone());
        engine.end_leftover();
        let stats = Recorder::new(Some(paths.perf_log()));
        stats.restore_speeds();
        let app = Arc::new(Self {
            stats,
            paths,
            cfg: RwLock::new(cfg),
            manifests,
            detected: RwLock::new(BTreeMap::new()),
            parts: RwLock::new(BTreeMap::new()),
            attached: RwLock::new(vec![]),
            pending: RwLock::new(Default::default()),
            scan_begun: std::sync::atomic::AtomicBool::new(false),
            first_run: std::sync::atomic::AtomicBool::new(first_run),
            library: RwLock::new(Library::default()),
            engine,
            gate: Arc::new(tokio::sync::RwLock::new(())),
            http,
            os_version: host::os_version(),
            total_memory: host::total_memory(),
            also_loaded: RwLock::new(vec![]),
            quit: tokio::sync::Notify::new(),
            aliases: RwLock::new(vec![]),
            live_tokens: AtomicU64::new(0),
            monitor: std::sync::Mutex::new(std::collections::VecDeque::new()),
            last_activity: AtomicU64::new(now()),
            started: Instant::now(),
            import_external: opts.import_external,
            housekeeping_every: opts.housekeeping_every,
        });
        if opts.wait_for_scan {
            app.rescan().await;
        }
        Ok(app)
    }

    pub fn config(&self) -> Config {
        self.cfg.read().unwrap().clone()
    }

    pub fn manifest(&self, id: &str) -> Option<&Manifest> {
        self.manifests.iter().find(|m| m.id == id)
    }

    pub fn touch(&self) {
        self.last_activity.store(now(), Ordering::Relaxed);
    }

    pub fn plan(&self, sel: &Selection) -> Result<Plan, String> {
        engine::plan(sel, &self.manifests, &self.library.read().unwrap(), &self.config(), &self.paths, &self.os_version)
    }

    fn loader_version(&self, id: &str) -> Option<String> {
        self.detected.read().unwrap().get(id).and_then(|d| d.version.clone())
    }

    /// The library as it stands: what each place on disk held, in order of precedence, then what
    /// the attached servers list.
    fn publish_library(&self) {
        let mut lib = {
            let parts = self.parts.read().unwrap();
            library::merge(library::sources(self.import_external).into_iter().filter_map(|s| parts.get(s)))
        };
        for (id, name, build) in self.attached.read().unwrap().iter() {
            lib.add(id, name, build.clone());
        }
        lib.sort();
        *self.library.write().unwrap() = lib;
    }

    /// Ask every attached server (Ollama, LM Studio, Strata) which models it has, and note which of
    /// the earlier Home apps have a model loaded.
    pub async fn refresh_attached(&self) {
        let cfg = self.config();
        let mut found = vec![];
        for m in self.manifests.iter().filter(|m| m.kind == Kind::Attach && m.supports_platform()) {
            let Some(attach) = &m.attach else { continue };
            let base = cfg.loader(&m.id).base_url.unwrap_or(attach.base_url.clone());
            let url = format!("{}/models", base.trim_end_matches('/'));
            let ids: Option<Vec<String>> = async {
                let body: Value = self.http.get(&url).timeout(Duration::from_millis(1500)).send().await.ok()?.error_for_status().ok()?.json().await.ok()?;
                Some(body.get("data")?.as_array()?.iter().filter_map(|x| x.get("id")?.as_str().map(String::from)).collect())
            }
            .await;
            self.detected.write().unwrap().entry(m.id.clone()).or_default().reachable = Some(ids.is_some());
            for remote in ids.unwrap_or_default() {
                let format = m.formats.first().cloned().unwrap_or(m.id.clone());
                found.push((library::model_id(&remote), library::display_name(&remote), Build { format, reference: remote.clone(), loader: Some(m.id.clone()), source: m.id.clone(), ..Default::default() }));
            }
        }
        *self.attached.write().unwrap() = found;
        self.publish_library();

        let mut loaded = vec![];
        if self.import_external {
            for (app, port) in crate::import::home_app_engines() {
                let answering = self.http.get(format!("http://127.0.0.1:{port}/v1/models")).timeout(Duration::from_millis(600)).send().await.is_ok_and(|r| r.status().is_success());
                if answering {
                    loaded.push(app.to_string());
                }
            }
        }
        *self.also_loaded.write().unwrap() = loaded;
    }

    /// Look (again) for loaders and models. Every place models are read from is read on its own and
    /// shows up in the library as soon as it has answered. Returns when all of them have.
    pub async fn rescan(self: &Arc<Self>) {
        self.scan_begun.store(true, Ordering::Relaxed);
        let (p, m, c) = (self.paths.clone(), self.manifests.clone(), self.config());
        let detect = tokio::task::spawn_blocking(move || engine::detect(&m, &c, &p));
        let mut reads = vec![];
        for source in library::sources(self.import_external) {
            self.pending.write().unwrap().insert(source.to_string());
            let (app, paths) = (self.clone(), self.paths.clone());
            reads.push(tokio::spawn(async move {
                if let Ok(part) = tokio::task::spawn_blocking(move || library::scan_source(source, &paths)).await {
                    app.parts.write().unwrap().insert(source.to_string(), part);
                }
                app.pending.write().unwrap().remove(source);
                app.publish_library();
            }));
        }
        if let Ok(mut found) = detect.await {
            let mut g = self.detected.write().unwrap();
            for (id, d) in found.iter_mut() {
                d.reachable = g.get(id).and_then(|old| old.reachable);
            }
            *g = found;
        }
        self.refresh_attached().await;
        for read in reads {
            let _ = read.await;
        }
        // on the very first run, take over what uzu Home or TensorFold Home had selected
        if self.first_run.swap(false, Ordering::Relaxed) && self.import_external && !self.config().selection.is_set() {
            let lib = self.library.read().unwrap().clone();
            let mut cfg = self.config();
            if let Some(sel) = crate::import::selection_from_home_apps(&lib, &mut cfg) {
                eprintln!("8gent: first run: taking over {} on {} from the Home app", sel.model, sel.loader);
                let taken = {
                    let mut g = self.cfg.write().unwrap();
                    let free = !g.selection.is_set();
                    if free {
                        g.loaders = cfg.loaders;
                        g.selection = sel.clone();
                        let _ = g.save(&self.paths);
                    }
                    free
                };
                // "keep the model loaded" was the Home app's setting: load it now, as the Home app would have
                if taken && self.config().memory_mode == MemoryMode::Always {
                    let app = self.clone();
                    tokio::spawn(async move {
                        if let Err(e) = app.acquire(&sel).await {
                            eprintln!("8gent: could not load {}: {}", sel.model, normalize::error_message(&e.body).unwrap_or_default());
                        }
                    });
                }
            }
        }
    }

    /// The loaders that can run `model` here, in the order of the loader list.
    pub fn loaders_for(&self, model: &str) -> Vec<&Manifest> {
        let cfg = self.config();
        self.manifests.iter().filter(|m| self.plan(&Selection { loader: m.id.clone(), model: model.to_string(), context: 0, thinking: cfg.selection.thinking }).is_ok()).collect()
    }

    /// What a request's `model` means: the served name (or any unknown name) is the selection;
    /// "loader/model" and a library model's id name something specific.
    pub fn resolve(&self, model_field: Option<&str>) -> Result<Target, ApiErr> {
        let cfg = self.config();
        let name = model_field.unwrap_or("").trim();
        let requested = if name.is_empty() { cfg.served_name.clone() } else { name.to_string() };
        let mut sel = cfg.selection.clone();
        let mut explicit = false;
        if let Some((loader, model)) = name.split_once('/') {
            if self.manifest(loader).is_some() {
                explicit = sel.loader != loader || sel.model != model;
                if explicit {
                    sel = Selection { loader: loader.into(), model: model.into(), context: 0, thinking: sel.thinking };
                }
            }
        } else if !name.is_empty() && name != cfg.served_name && name != sel.model && self.library.read().unwrap().get(name).is_some() {
            let loaders = self.loaders_for(name);
            let loader = loaders.iter().find(|m| m.id == sel.loader).or(loaders.first()).ok_or_else(|| ApiErr::unavailable(format!("No loader installed here can run {name}.")))?;
            sel = Selection { loader: loader.id.clone(), model: name.into(), context: 0, thinking: sel.thinking };
            explicit = true;
        }
        if !sel.is_set() {
            return Err(ApiErr::unavailable("Nothing is selected yet. Choose a model and loader in 8gent (or run `8gent use <model>`)."));
        }
        if explicit {
            if !cfg.auto_switch {
                return Err(ApiErr::new(StatusCode::CONFLICT, format!("{requested} is not what is selected, and switching on request is turned off."), "invalid_request_error", Some("model_not_selected")));
            }
            self.plan(&sel).map_err(ApiErr::bad_request)?;
            let mut g = self.cfg.write().unwrap();
            g.selection = sel.clone();
            let _ = g.save(&self.paths);
        }
        Ok(Target { selection: sel, requested })
    }

    /// An engine that serves `sel`, started or switched to if needed, with the gate held shared so
    /// it stays up while the caller uses it.
    pub async fn acquire(&self, sel: &Selection) -> Result<(Running, tokio::sync::OwnedRwLockReadGuard<()>), ApiErr> {
        for _ in 0..4 {
            let plan = self.plan(sel).map_err(ApiErr::unavailable)?;
            let guard = self.gate.clone().read_owned().await;
            if let Some(run) = self.engine.current().filter(|r| r.serves(&plan)) {
                return Ok((run, guard));
            }
            drop(guard);
            let _alone = self.gate.clone().write_owned().await;
            if self.engine.current().is_some_and(|r| r.serves(&plan)) {
                continue;
            }
            self.engine.stop().await;
            let version = self.loader_version(&plan.loader);
            self.engine.start(plan, version).await.map_err(ApiErr::unavailable)?;
        }
        Err(ApiErr::unavailable("The engine kept changing while this request waited. Try again."))
    }

    /// Change what is selected. With `load`, the new engine must come up, or everything goes back to
    /// how it was (and what was running is started again).
    pub async fn select(&self, new: Selection, load: Option<bool>) -> Result<Value, ApiErr> {
        let plan = self.plan(&new).map_err(ApiErr::bad_request)?;
        let _alone = self.gate.clone().write_owned().await;
        let previous = self.config().selection;
        let was_running = self.engine.current();
        let load = load.unwrap_or(was_running.is_some() || self.config().memory_mode == MemoryMode::Always);
        if was_running.as_ref().is_some_and(|r| r.serves(&plan)) {
            self.save_selection(&new)?;
            return Ok(json!({"ok": true, "selection": new, "loaded": true, "restarted": false}));
        }
        self.engine.stop().await;
        self.engine.mark_stopped();
        self.save_selection(&new)?;
        if !load {
            return Ok(json!({"ok": true, "selection": new, "loaded": false}));
        }
        let version = self.loader_version(&plan.loader);
        match self.engine.start(plan, version).await {
            Ok(run) => Ok(json!({"ok": true, "selection": new, "loaded": true, "restarted": true, "load_seconds": run.load_seconds})),
            Err(reason) => {
                self.save_selection(&previous)?;
                let mut restored = false;
                if was_running.is_some() && previous.is_set() {
                    if let Ok(old_plan) = self.plan(&previous) {
                        let v = self.loader_version(&old_plan.loader);
                        restored = self.engine.start(old_plan, v).await.is_ok();
                    }
                }
                if !restored {
                    self.engine.mark_stopped();
                }
                // the sentence first; the engine's own last output, if any, goes in `detail`
                let (head, detail) = reason.split_once("\n\n").unwrap_or((&reason, ""));
                let went_back = if previous.is_set() {
                    let lib = self.library.read().unwrap();
                    let model = lib.get(&previous.model).map(|m| m.name.clone()).unwrap_or(previous.model.clone());
                    let loader = self.manifest(&previous.loader).map(|m| m.name.clone()).unwrap_or(previous.loader.clone());
                    format!(" 8gent went back to {model} on {loader}.")
                } else {
                    String::new()
                };
                let mut body = error_body(format!("{head}{went_back}"), "switch_failed", Some("switch_rolled_back"));
                body["error"]["detail"] = json!(detail);
                body["rolled_back"] = json!(true);
                body["restored"] = json!(restored);
                body["selection"] = json!(previous);
                Err(ApiErr { status: StatusCode::CONFLICT, body })
            }
        }
    }

    fn save_selection(&self, sel: &Selection) -> Result<(), ApiErr> {
        let mut g = self.cfg.write().unwrap();
        g.selection = sel.clone();
        if sel.context > 0 {
            g.contexts.insert(sel.model.clone(), sel.context);
        }
        g.save(&self.paths).map_err(|e| ApiErr::new(StatusCode::INTERNAL_SERVER_ERROR, e, "server_error", None))
    }

    /// Once a second: how fast answers are being written, how many requests are in progress, and
    /// the memory the engine holds. Cheap: two counters and one system call.
    pub async fn sampler(self: Arc<Self>) {
        let mut last = self.live_tokens.load(Ordering::Relaxed);
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let total = self.live_tokens.load(Ordering::Relaxed);
            let state = self.engine.state();
            let pid = self.engine.current().and_then(|r| r.pid);
            let sample = Sample { at: now(), tokens_per_second: total.saturating_sub(last) as f64, in_flight: self.stats.in_flight(), memory_bytes: pid.and_then(engine::process_memory), state };
            last = total;
            let mut g = self.monitor.lock().unwrap();
            g.push_back(sample);
            while g.len() > 180 {
                g.pop_front();
            }
        }
    }

    /// Every few seconds: notice an engine that died, unload an idle one, refresh attached servers.
    pub async fn housekeeping(self: Arc<Self>) {
        let mut last_refresh = Instant::now();
        loop {
            tokio::time::sleep(self.housekeeping_every).await;
            let cfg = self.config();
            let running = self.engine.current();
            let idle_s = now().saturating_sub(self.last_activity.load(Ordering::Relaxed));
            if let Some(run) = running {
                if run.managed && cfg.memory_mode == MemoryMode::Idle && cfg.idle_minutes > 0.0 && idle_s as f64 >= cfg.idle_minutes * 60.0 && self.stats.in_flight() == 0 {
                    if let Ok(_alone) = self.gate.clone().try_write_owned() {
                        eprintln!("8gent: unloading {} after {idle_s} s without a request", run.model);
                        self.engine.stop().await;
                    }
                }
            }
            if last_refresh.elapsed() >= Duration::from_secs(30) {
                last_refresh = Instant::now();
                self.refresh_attached().await;
            }
        }
    }

    // ------------------------------------------------------------------------------------- payloads

    fn requirement_rows(&self, m: &Manifest, version: Option<&str>) -> Vec<Value> {
        m.requires
            .iter()
            .filter(|r| r.os == std::env::consts::OS)
            .map(|r| {
                let applies = match (&r.since, version) {
                    (Some(since), Some(v)) => compare_versions(v, since) != std::cmp::Ordering::Less,
                    _ => true,
                };
                let met = compare_versions(&self.os_version, &r.min) != std::cmp::Ordering::Less;
                json!({"os": r.os, "min": r.min, "since": r.since, "note": r.note, "applies_to_installed": applies, "met": met})
            })
            .collect()
    }

    pub fn loaders_payload(&self) -> Value {
        let cfg = self.config();
        let detected = self.detected.read().unwrap().clone();
        let lib = self.library.read().unwrap().clone();
        let rows: Vec<Value> = self
            .manifests
            .iter()
            .map(|m| {
                let d = detected.get(&m.id).cloned().unwrap_or_default();
                let installed = match m.kind {
                    Kind::Managed => d.program.is_some(),
                    Kind::Attach => d.reachable == Some(true),
                };
                let models: Vec<&str> = lib.models.iter().filter(|x| engine::build_for(&lib, &x.id, m).is_some()).map(|x| x.id.as_str()).collect();
                json!({
                    "id": m.id, "name": m.name, "description": m.description, "homepage": m.homepage, "kind": m.kind,
                    "formats": m.formats, "supported_here": m.supports_platform(), "installed": installed,
                    "program": d.program, "version": d.version, "reachable": d.reachable,
                    "capabilities": m.capabilities, "requirements": self.requirement_rows(m, d.version.as_deref()),
                    "variants": m.variants.keys().collect::<Vec<_>>(), "update": m.update,
                    "models": models, "selected": cfg.selection.loader == m.id,
                })
            })
            .collect();
        json!({"loaders": rows})
    }

    /// Weights, about 1.1 GB of runtime, and the KV cache for the window (every conversation a
    /// loader keeps has its own).
    fn estimate(build: &Build, context: u32, sessions: u32) -> Option<u64> {
        let kv = build.kv_bytes_per_token? * context as u64 * sessions.max(1) as u64;
        Some(build.size_bytes? + RUNTIME_BYTES + kv)
    }

    pub fn models_payload(&self) -> Value {
        let cfg = self.config();
        let lib = self.library.read().unwrap().clone();
        let running = self.engine.current();
        let rows: Vec<Value> = lib
            .models
            .iter()
            .map(|model| {
                let loaders: Vec<Value> = self
                    .manifests
                    .iter()
                    .filter_map(|m| {
                        let build = engine::build_for(&lib, &model.id, m)?;
                        let sel = Selection { loader: m.id.clone(), model: model.id.clone(), context: 0, thinking: false };
                        let plan = self.plan(&sel);
                        // the window this pair would load with: remembered for the model, else the default
                        let context = plan.as_ref().map(|p| p.context).unwrap_or_else(|_| engine::default_context(build));
                        let sessions = cfg.loader(&m.id).sessions.unwrap_or(if m.capabilities.sessions > 0 { 1 } else { 0 }).max(1);
                        let managed = m.kind == Kind::Managed;
                        let version = self.loader_version(&m.id);
                        Some(json!({
                            "id": m.id, "name": m.name, "kind": m.kind, "version": version,
                            "runnable": plan.is_ok(), "why_not": plan.err(),
                            "serving": running.as_ref().is_some_and(|r| r.loader == m.id && r.model == model.id),
                            "selected": cfg.selection.loader == m.id && cfg.selection.model == model.id,
                            "format": build.format, "quant": build.quant, "variant": build.variant, "size_bytes": build.size_bytes,
                            "context": if managed { Some(context) } else { None }, "max_context": build.max_context,
                            // memory = base + kv x window x conversations, so a client can price any window
                            "base_bytes": build.size_bytes.filter(|_| managed).map(|b| b + RUNTIME_BYTES),
                            "kv_bytes_per_token": build.kv_bytes_per_token.filter(|_| managed), "sessions": sessions,
                            "estimated_memory_bytes": Self::estimate(build, context, sessions).filter(|_| managed),
                            "capabilities": m.capabilities,
                            "speed": self.stats.speed_for(&m.id, &model.id),
                        }))
                    })
                    .collect();
                json!({"id": model.id, "name": model.name, "loaders": loaders, "builds": model.builds, "selected": cfg.selection.model == model.id})
            })
            .collect();
        json!({"models": rows, "memory": {"total_bytes": self.total_memory, "gpu_budget_bytes": host::gpu_budget(self.total_memory)}})
    }

    pub fn status_payload(&self) -> Value {
        let cfg = self.config();
        let running = self.engine.current();
        let state = self.engine.state();
        let memory = running.as_ref().and_then(|r| r.pid).and_then(engine::process_memory);
        let loading = self.engine.loading().map(|(loader, model, s)| json!({"loader": loader, "model": model, "seconds": (s * 10.0).round() / 10.0}));
        json!({
            "name": "8gent", "version": crate::VERSION, "platform": manifest::platform(), "os_version": self.os_version,
            "port": cfg.port, "also_listening": self.aliases.read().unwrap().clone(), "also_listen": cfg.also_listen,
            "login_item": crate::service::installed(), "share": cfg.share, "key_set": cfg.api_key.as_deref().is_some_and(|k| !k.is_empty()),
            "served_name": cfg.served_name, "memory_mode": cfg.memory_mode, "idle_minutes": cfg.idle_minutes, "auto_switch": cfg.auto_switch,
            "home": self.paths.home.to_string_lossy(),
            "scanning": self.pending.read().unwrap().iter().collect::<Vec<_>>(),
            "also_loaded": self.also_loaded.read().unwrap().iter().map(|app| json!({"app": app, "detail": "has a model loaded"})).collect::<Vec<_>>(),
            "uptime_s": self.started.elapsed().as_secs(),
            "idle_s": now().saturating_sub(self.last_activity.load(Ordering::Relaxed)),
            "selection": cfg.selection,
            "engine": {"state": state, "running": running, "memory_bytes": memory, "loading": loading,
                       "error": if state == EngineState::Error { self.engine.last_error() } else { None }},
            "memory": {"total_bytes": self.total_memory, "gpu_budget_bytes": host::gpu_budget(self.total_memory)},
            "stats": self.stats.snapshot(20),
        })
    }

    /// `/v1/models`: the served name first, then every loader and model pair that can run here, each
    /// with the context window a client may fill.
    pub fn openai_models(&self) -> Value {
        let cfg = self.config();
        let lib = self.library.read().unwrap().clone();
        let mut data = vec![];
        let caps = |m: &Manifest| json!({"tools": m.capabilities.tools, "json_schema": m.capabilities.json_schema, "vision": m.capabilities.vision});
        if cfg.selection.is_set() {
            if let (Some(m), Ok(plan)) = (self.manifest(&cfg.selection.loader), self.plan(&cfg.selection)) {
                data.push(json!({"id": cfg.served_name, "object": "model", "owned_by": "8gent", "context_length": plan.context,
                    "max_context_length": plan.build.max_context, "loader": m.id, "model": plan.model, "capabilities": caps(m)}));
            }
        }
        for model in &lib.models {
            for m in &self.manifests {
                let sel = Selection { loader: m.id.clone(), model: model.id.clone(), context: 0, thinking: false };
                if let Ok(plan) = self.plan(&sel) {
                    let installed = match m.kind {
                        Kind::Managed => true,
                        Kind::Attach => self.detected.read().unwrap().get(&m.id).and_then(|d| d.reachable) == Some(true),
                    };
                    if installed {
                        data.push(json!({"id": format!("{}/{}", m.id, model.id), "object": "model", "owned_by": m.id, "context_length": plan.context,
                            "max_context_length": plan.build.max_context, "loader": m.id, "model": model.id, "capabilities": caps(m)}));
                    }
                }
            }
        }
        json!({"object": "list", "data": data})
    }
}

// --------------------------------------------------------------------------------------------- routes

type S = State<Arc<App>>;

pub fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/health", get(|| async { Json(json!({"status": "ok"})) }))
        .route("/v1/models", get(models))
        .route("/8gent/v1/status", get(status))
        .route("/8gent/v1/loaders", get(loaders))
        .route("/8gent/v1/models", get(library_models))
        .route("/8gent/v1/logs", get(logs))
        .route("/8gent/v1/requests", get(requests))
        .route("/8gent/v1/monitor", get(monitor))
        .route("/8gent/v1/chats", get(chats))
        .route("/8gent/v1/chats/{id}", get(chat_get).put(chat_put).delete(chat_delete))
        .route("/8gent/v1/settings", post(settings))
        .route("/8gent/v1/select", post(select))
        .route("/8gent/v1/engine/start", post(engine_start))
        .route("/8gent/v1/engine/stop", post(engine_stop))
        .route("/8gent/v1/rescan", post(rescan))
        .route("/8gent/v1/shutdown", post(shutdown))
        .route("/8gent/v1/login-item", post(login_item))
        .fallback(forward)
        .layer(DefaultBodyLimit::disable())
        .with_state(app)
}

/// Requests from this computer are trusted. From another computer, the endpoint needs the key when one
/// is set, and the control API always needs one.
pub fn allowed(from_this_computer: bool, key: Option<&str>, given: Option<&str>, control: bool) -> bool {
    if from_this_computer {
        return true;
    }
    match key.filter(|k| !k.is_empty()) {
        Some(k) => given == Some(k),
        None => !control,
    }
}

fn authorize(app: &App, remote: &SocketAddr, headers: &HeaderMap, control: bool) -> Result<(), ApiErr> {
    let cfg = app.config();
    let given = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .or_else(|| headers.get("x-api-key").and_then(|v| v.to_str().ok()));
    if allowed(remote.ip().is_loopback(), cfg.api_key.as_deref(), given, control) {
        Ok(())
    } else {
        Err(ApiErr::new(StatusCode::UNAUTHORIZED, "This endpoint needs its API key from other computers.", "authentication_error", Some("invalid_api_key")))
    }
}

async fn models(State(app): S, ConnectInfo(remote): ConnectInfo<SocketAddr>, headers: HeaderMap) -> Result<Json<Value>, ApiErr> {
    authorize(&app, &remote, &headers, false)?;
    Ok(Json(app.openai_models()))
}

async fn status(State(app): S, ConnectInfo(remote): ConnectInfo<SocketAddr>, headers: HeaderMap) -> Result<Json<Value>, ApiErr> {
    authorize(&app, &remote, &headers, true)?;
    Ok(Json(app.status_payload()))
}

async fn loaders(State(app): S, ConnectInfo(remote): ConnectInfo<SocketAddr>, headers: HeaderMap) -> Result<Json<Value>, ApiErr> {
    authorize(&app, &remote, &headers, true)?;
    Ok(Json(app.loaders_payload()))
}

async fn library_models(State(app): S, ConnectInfo(remote): ConnectInfo<SocketAddr>, headers: HeaderMap) -> Result<Json<Value>, ApiErr> {
    authorize(&app, &remote, &headers, true)?;
    Ok(Json(app.models_payload()))
}

async fn logs(State(app): S, ConnectInfo(remote): ConnectInfo<SocketAddr>, headers: HeaderMap, Query(q): Query<BTreeMap<String, String>>) -> Result<Json<Value>, ApiErr> {
    authorize(&app, &remote, &headers, true)?;
    let loader = q.get("loader").cloned().unwrap_or_else(|| app.config().selection.loader);
    if app.manifest(&loader).is_none() {
        return Err(ApiErr::bad_request(format!("There is no loader called \"{loader}\".")));
    }
    let lines: usize = q.get("lines").and_then(|n| n.parse().ok()).unwrap_or(200).min(5000);
    let text = std::fs::read_to_string(app.paths.engine_log(&loader)).unwrap_or_default();
    let all: Vec<&str> = text.lines().collect();
    Ok(Json(json!({"loader": loader, "lines": all[all.len().saturating_sub(lines)..]})))
}

async fn requests(State(app): S, ConnectInfo(remote): ConnectInfo<SocketAddr>, headers: HeaderMap, Query(q): Query<BTreeMap<String, String>>) -> Result<Json<Value>, ApiErr> {
    authorize(&app, &remote, &headers, true)?;
    let limit: usize = q.get("limit").and_then(|n| n.parse().ok()).unwrap_or(100).min(200);
    Ok(Json(json!({"requests": app.stats.recent(limit)})))
}

async fn monitor(State(app): S, ConnectInfo(remote): ConnectInfo<SocketAddr>, headers: HeaderMap) -> Result<Json<Value>, ApiErr> {
    authorize(&app, &remote, &headers, true)?;
    let samples: Vec<Sample> = app.monitor.lock().unwrap().iter().cloned().collect();
    Ok(Json(json!({"samples": samples, "gpu_budget_bytes": host::gpu_budget(app.total_memory), "total_bytes": app.total_memory})))
}

// --------------------------------------------------------------------------------------------- chats
// Conversations held in 8gent's own Chat screen, one JSON file each in <home>/chats. Requests that
// other programs send to the endpoint are never stored here.

fn chat_file(app: &App, id: &str) -> Result<std::path::PathBuf, ApiErr> {
    let ok = !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if !ok {
        return Err(ApiErr::bad_request("A chat's id is letters, digits and dashes."));
    }
    Ok(app.paths.home.join("chats").join(format!("{id}.json")))
}

async fn chats(State(app): S, ConnectInfo(remote): ConnectInfo<SocketAddr>, headers: HeaderMap) -> Result<Json<Value>, ApiErr> {
    authorize(&app, &remote, &headers, true)?;
    let mut rows: Vec<Value> = std::fs::read_dir(app.paths.home.join("chats"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let doc: Value = serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok()?;
            Some(json!({"id": doc.get("id")?, "title": doc.get("title")?, "updated_at": doc.get("updated_at")?, "messages": doc.get("messages")?.as_array()?.len()}))
        })
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r["updated_at"].as_u64().unwrap_or(0)));
    Ok(Json(json!({"chats": rows})))
}

async fn chat_get(State(app): S, ConnectInfo(remote): ConnectInfo<SocketAddr>, headers: HeaderMap, axum::extract::Path(id): axum::extract::Path<String>) -> Result<Json<Value>, ApiErr> {
    authorize(&app, &remote, &headers, true)?;
    let text = std::fs::read_to_string(chat_file(&app, &id)?).map_err(|_| ApiErr::new(StatusCode::NOT_FOUND, "There is no such chat.", "invalid_request_error", Some("not_found")))?;
    serde_json::from_str(&text).map(Json).map_err(|e| ApiErr::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), "server_error", None))
}

async fn chat_put(State(app): S, ConnectInfo(remote): ConnectInfo<SocketAddr>, headers: HeaderMap, axum::extract::Path(id): axum::extract::Path<String>, Json(mut body): Json<Value>) -> Result<Json<Value>, ApiErr> {
    authorize(&app, &remote, &headers, true)?;
    let file = chat_file(&app, &id)?;
    if !body.get("messages").is_some_and(|m| m.is_array()) {
        return Err(ApiErr::bad_request("A chat needs its messages."));
    }
    body["id"] = json!(id);
    body["updated_at"] = json!(now());
    if !body.get("title").is_some_and(|t| t.is_string()) {
        body["title"] = json!("New chat");
    }
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(file.parent().unwrap())?;
        let tmp = file.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(&body).unwrap_or_default())?;
        std::fs::rename(&tmp, &file)
    };
    write().map_err(|e| ApiErr::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), "server_error", None))?;
    Ok(Json(json!({"ok": true, "id": id, "updated_at": body["updated_at"]})))
}

async fn chat_delete(State(app): S, ConnectInfo(remote): ConnectInfo<SocketAddr>, headers: HeaderMap, axum::extract::Path(id): axum::extract::Path<String>) -> Result<Json<Value>, ApiErr> {
    authorize(&app, &remote, &headers, true)?;
    let _ = std::fs::remove_file(chat_file(&app, &id)?);
    Ok(Json(json!({"ok": true})))
}

/// Change how the service behaves: `{memory_mode?, idle_minutes?, auto_switch?}`. Saved at once.
async fn settings(State(app): S, ConnectInfo(remote): ConnectInfo<SocketAddr>, headers: HeaderMap, Json(body): Json<Value>) -> Result<Json<Value>, ApiErr> {
    authorize(&app, &remote, &headers, true)?;
    let mut cfg = app.config();
    if let Some(mode) = body.get("memory_mode") {
        cfg.memory_mode = serde_json::from_value(mode.clone()).map_err(|_| ApiErr::bad_request("memory_mode is always, idle or on_demand."))?;
    }
    if let Some(minutes) = body.get("idle_minutes") {
        cfg.idle_minutes = minutes.as_f64().filter(|m| *m >= 0.0 && *m <= 24.0 * 60.0).ok_or_else(|| ApiErr::bad_request("idle_minutes is a number of minutes, 0 for never."))?;
    }
    if let Some(auto) = body.get("auto_switch") {
        cfg.auto_switch = auto.as_bool().ok_or_else(|| ApiErr::bad_request("auto_switch is true or false."))?;
    }
    cfg.save(&app.paths).map_err(|e| ApiErr::new(StatusCode::INTERNAL_SERVER_ERROR, e, "server_error", None))?;
    // the selection may have changed while this request was read: keep the live one
    let mut g = app.cfg.write().unwrap();
    let (selection, contexts) = (g.selection.clone(), g.contexts.clone());
    *g = cfg;
    g.selection = selection;
    g.contexts = contexts;
    let _ = g.save(&app.paths);
    Ok(Json(json!({"ok": true, "memory_mode": g.memory_mode, "idle_minutes": g.idle_minutes, "auto_switch": g.auto_switch})))
}

async fn select(State(app): S, ConnectInfo(remote): ConnectInfo<SocketAddr>, headers: HeaderMap, Json(body): Json<Value>) -> Result<Json<Value>, ApiErr> {
    authorize(&app, &remote, &headers, true)?;
    let current = app.config().selection;
    let text = |k: &str| body.get(k).and_then(|v| v.as_str()).map(String::from);
    let model = text("model").unwrap_or(current.model.clone());
    if model.is_empty() {
        return Err(ApiErr::bad_request("Name a model to select."));
    }
    let loader = match text("loader") {
        Some(l) => l,
        // only the model was named: keep the loader if it can run it, else the first that can
        None => {
            let able = app.loaders_for(&model);
            able.iter().find(|m| m.id == current.loader).or(able.first()).map(|m| m.id.clone()).ok_or_else(|| {
                let known = app.library.read().unwrap().get(&model).is_some();
                ApiErr::bad_request(if known { format!("No loader installed here can run {model}.") } else { format!("There is no model called \"{model}\" in the library.") })
            })?
        }
    };
    // the window belongs to the model: another loader for the same model keeps it, another model starts from its default
    let context = body.get("context").and_then(|v| v.as_u64()).map(|n| n as u32).unwrap_or(if model == current.model { current.context } else { 0 });
    let thinking = body.get("thinking").and_then(|v| v.as_bool()).unwrap_or(current.thinking);
    let load = body.get("load").and_then(|v| v.as_bool());
    app.select(Selection { loader, model, context, thinking }, load).await.map(Json)
}

async fn engine_start(State(app): S, ConnectInfo(remote): ConnectInfo<SocketAddr>, headers: HeaderMap) -> Result<Json<Value>, ApiErr> {
    authorize(&app, &remote, &headers, true)?;
    let sel = app.config().selection;
    if !sel.is_set() {
        return Err(ApiErr::bad_request("Nothing is selected yet."));
    }
    app.touch();
    let (run, _guard) = app.acquire(&sel).await?;
    Ok(Json(json!({"ok": true, "running": run})))
}

async fn engine_stop(State(app): S, ConnectInfo(remote): ConnectInfo<SocketAddr>, headers: HeaderMap) -> Result<Json<Value>, ApiErr> {
    authorize(&app, &remote, &headers, true)?;
    let _alone = app.gate.clone().write_owned().await;
    app.engine.stop().await;
    app.engine.mark_stopped();
    Ok(Json(json!({"ok": true})))
}

async fn rescan(State(app): S, ConnectInfo(remote): ConnectInfo<SocketAddr>, headers: HeaderMap) -> Result<Json<Value>, ApiErr> {
    authorize(&app, &remote, &headers, true)?;
    // a place that is waiting for a permission answer must not hold this call: report it instead
    let _ = tokio::time::timeout(Duration::from_secs(10), app.rescan()).await;
    let still: Vec<String> = app.pending.read().unwrap().iter().cloned().collect();
    Ok(Json(json!({"ok": true, "models": app.library.read().unwrap().models.len(), "still_reading": still})))
}

/// Stop the service (and with it the engine). The answer goes out first.
async fn shutdown(State(app): S, ConnectInfo(remote): ConnectInfo<SocketAddr>, headers: HeaderMap) -> Result<Json<Value>, ApiErr> {
    authorize(&app, &remote, &headers, true)?;
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        app.quit.notify_waiters();
    });
    Ok(Json(json!({"ok": true})))
}

/// Start at login, or stop doing so: `{enabled}`. Switching it on while the service was started some
/// other way hands the service over to the login agent: this one stops and the agent's starts.
async fn login_item(State(app): S, ConnectInfo(remote): ConnectInfo<SocketAddr>, headers: HeaderMap, Json(body): Json<Value>) -> Result<Json<Value>, ApiErr> {
    authorize(&app, &remote, &headers, true)?;
    let enabled = body.get("enabled").and_then(|v| v.as_bool()).ok_or_else(|| ApiErr::bad_request("enabled is true or false."))?;
    let fail = |e: String| ApiErr::new(StatusCode::INTERNAL_SERVER_ERROR, e, "server_error", None);
    if !enabled {
        crate::service::remove(false).map_err(fail)?;
        return Ok(Json(json!({"ok": true, "login_item": false})));
    }
    let program = std::env::current_exe().map_err(|e| fail(e.to_string()))?;
    crate::service::write(&program, &app.paths).map_err(fail)?;
    let handing_over = !crate::service::started_at_login();
    if handing_over {
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            // the agent's service takes the port once this one has let go of it
            let _ = std::process::Command::new("/bin/sh").args(["-c", "sleep 3; /bin/launchctl bootstrap gui/$(id -u) \"$0\"", &crate::service::plist_path().to_string_lossy()]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
            app.quit.notify_waiters();
        });
    }
    Ok(Json(json!({"ok": true, "login_item": true, "restarting": handing_over})))
}

/// Everything else under /v1 goes to the engine; other paths do not exist.
async fn forward(State(app): S, ConnectInfo(remote): ConnectInfo<SocketAddr>, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Response {
    if let Err(e) = authorize(&app, &remote, &headers, false) {
        return e.into_response();
    }
    let path = uri.path().to_string();
    if !path.starts_with("/v1/") {
        return ApiErr::new(StatusCode::NOT_FOUND, format!("8gent has no {path}. The model endpoint is under /v1, the control API under /8gent/v1."), "invalid_request_error", Some("not_found")).into_response();
    }
    let path_and_query = uri.path_and_query().map(|p| p.as_str().to_string()).unwrap_or(path);
    proxy::handle(app, method, path_and_query, headers, body).await
}

/// Run the service on `listener` until `shutdown` resolves, then stop the engine.
pub async fn serve(app: Arc<App>, listener: tokio::net::TcpListener, shutdown: impl Future<Output = ()> + Send + 'static) -> Result<(), String> {
    tokio::spawn(app.clone().housekeeping());
    tokio::spawn(app.clone().sampler());
    if !app.scan_begun.load(Ordering::Relaxed) {
        let looking = app.clone();
        tokio::spawn(async move { looking.rescan().await });
    }
    if app.config().memory_mode == MemoryMode::Always && app.config().selection.is_set() {
        let a = app.clone();
        tokio::spawn(async move {
            let sel = a.config().selection;
            if let Err(e) = a.acquire(&sel).await {
                eprintln!("8gent: could not load {} at start: {}", sel.model, normalize::error_message(&e.body).unwrap_or_default());
            }
        });
    }
    let asked = app.clone();
    let stop = async move {
        tokio::select! {
            _ = shutdown => {}
            _ = asked.quit.notified() => {}
        }
    };
    let result = axum::serve(listener, router(app.clone()).into_make_service_with_connect_info::<SocketAddr>()).with_graceful_shutdown(stop).await;
    app.engine.stop().await;
    result.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::allowed;

    #[test]
    fn other_computers_need_the_key() {
        // this computer: always
        assert!(allowed(true, Some("k"), None, true));
        // another computer, a key is set: the endpoint and the control API both need it
        assert!(!allowed(false, Some("k"), None, false) && !allowed(false, Some("k"), Some("wrong"), false));
        assert!(allowed(false, Some("k"), Some("k"), false) && allowed(false, Some("k"), Some("k"), true));
        // another computer, no key set: the endpoint is open (the user chose to share it), the control API is not
        assert!(allowed(false, None, None, false) && !allowed(false, None, None, true) && !allowed(false, Some(""), None, true));
    }
}

/// Answer on one more port (the address of a Home app 8gent replaced). If something else holds the
/// port, try again every 10 s: the Home app may still be shutting down, or be opened again later.
pub async fn serve_alias(app: Arc<App>, addr: SocketAddr) {
    loop {
        if let Ok(listener) = tokio::net::TcpListener::bind(addr).await {
            eprintln!("8gent: also answering on http://{addr}/v1");
            app.aliases.write().unwrap().push(addr.port());
            let _ = axum::serve(listener, router(app.clone()).into_make_service_with_connect_info::<SocketAddr>()).await;
            app.aliases.write().unwrap().retain(|p| *p != addr.port());
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}
