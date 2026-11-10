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

pub struct Options {
    pub paths: Paths,
    /// Read the model lists of LM Studio and the earlier Home apps, and on the first run take over
    /// what uzu Home or TensorFold Home had selected. Tests switch it off.
    pub import_external: bool,
    /// How often the service looks for an idle or dead engine.
    pub housekeeping_every: Duration,
}

impl Options {
    pub fn new(paths: Paths) -> Self {
        Self { paths, import_external: true, housekeeping_every: Duration::from_secs(5) }
    }
}

pub struct App {
    pub paths: Paths,
    pub cfg: RwLock<Config>,
    pub manifests: Vec<Manifest>,
    pub detected: RwLock<BTreeMap<String, Detected>>,
    /// Models found on disk; attached servers' models are merged over it into `library`.
    disk_library: RwLock<Library>,
    pub library: RwLock<Library>,
    pub engine: Engine,
    /// Requests hold it shared while an engine answers them; starting, stopping or switching the
    /// engine takes it alone, so no request loses its engine half way.
    pub gate: Arc<tokio::sync::RwLock<()>>,
    pub stats: Recorder,
    pub http: reqwest::Client,
    pub os_version: String,
    pub total_memory: Option<u64>,
    last_activity: AtomicU64,
    started: Instant,
    import_external: bool,
    housekeeping_every: Duration,
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
    pub async fn build(opts: Options) -> Result<Arc<Self>, String> {
        let paths = opts.paths;
        paths.ensure().map_err(|e| format!("{}: {e}", paths.home.display()))?;
        let first_run = !paths.config().exists();
        let mut cfg = Config::load(&paths)?;
        let manifests = manifest::load_all(&paths)?;
        let http = reqwest::Client::builder().connect_timeout(Duration::from_secs(5)).pool_idle_timeout(Duration::from_secs(30)).build().map_err(|e| e.to_string())?;
        let (p, m, c) = (paths.clone(), manifests.clone(), cfg.clone());
        let import_external = opts.import_external;
        let (detected, disk) = tokio::task::spawn_blocking(move || (engine::detect(&m, &c, &p), library::scan(&p, import_external))).await.map_err(|e| e.to_string())?;
        if first_run && import_external {
            if let Some(sel) = crate::import::selection_from_home_apps(&disk, &mut cfg) {
                eprintln!("8gent: first run: taking over {} on {} from the Home app", sel.model, sel.loader);
                cfg.selection = sel;
            }
            cfg.save(&paths)?;
        }
        let engine = Engine::new(paths.clone(), http.clone());
        engine.end_leftover();
        let app = Arc::new(Self {
            stats: Recorder::new(Some(paths.perf_log())),
            paths,
            cfg: RwLock::new(cfg),
            manifests,
            detected: RwLock::new(detected),
            library: RwLock::new(disk.clone()),
            disk_library: RwLock::new(disk),
            engine,
            gate: Arc::new(tokio::sync::RwLock::new(())),
            http,
            os_version: host::os_version(),
            total_memory: host::total_memory(),
            last_activity: AtomicU64::new(now()),
            started: Instant::now(),
            import_external,
            housekeeping_every: opts.housekeeping_every,
        });
        app.refresh_attached().await;
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

    /// Ask every attached server (Ollama, LM Studio, Strata) which models it has, and put them in the
    /// library next to the ones on disk.
    pub async fn refresh_attached(&self) {
        let cfg = self.config();
        let mut lib = self.disk_library.read().unwrap().clone();
        for m in self.manifests.iter().filter(|m| m.kind == Kind::Attach && m.supports_platform()) {
            let Some(attach) = &m.attach else { continue };
            let base = cfg.loader(&m.id).base_url.unwrap_or(attach.base_url.clone());
            let url = format!("{}/models", base.trim_end_matches('/'));
            let ids: Option<Vec<String>> = async {
                let body: Value = self.http.get(&url).timeout(Duration::from_millis(1500)).send().await.ok()?.error_for_status().ok()?.json().await.ok()?;
                Some(body.get("data")?.as_array()?.iter().filter_map(|x| x.get("id")?.as_str().map(String::from)).collect())
            }
            .await;
            if let Some(d) = self.detected.write().unwrap().get_mut(&m.id) {
                d.reachable = Some(ids.is_some());
            }
            for remote in ids.unwrap_or_default() {
                let format = m.formats.first().cloned().unwrap_or(m.id.clone());
                lib.add(&library::model_id(&remote), &library::display_name(&remote), Build { format, reference: remote.clone(), loader: Some(m.id.clone()), source: m.id.clone(), ..Default::default() });
            }
        }
        lib.sort();
        *self.library.write().unwrap() = lib;
    }

    pub async fn rescan(&self) {
        let (p, m, c, import) = (self.paths.clone(), self.manifests.clone(), self.config(), self.import_external);
        if let Ok((detected, disk)) = tokio::task::spawn_blocking(move || (engine::detect(&m, &c, &p), library::scan(&p, import))).await {
            *self.detected.write().unwrap() = detected;
            *self.disk_library.write().unwrap() = disk;
        }
        self.refresh_attached().await;
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
                let mut body = error_body(format!("{reason} 8gent went back to {} on {}.", previous.model, previous.loader), "switch_failed", Some("switch_rolled_back"));
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
        Some(build.size_bytes? + 1_100_000_000 + kv)
    }

    pub fn models_payload(&self) -> Value {
        let cfg = self.config();
        let lib = self.library.read().unwrap().clone();
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
                        let context = engine::default_context(build);
                        let sessions = cfg.loader(&m.id).sessions.unwrap_or(1);
                        Some(json!({
                            "id": m.id, "name": m.name, "runnable": plan.is_ok(), "why_not": plan.err(),
                            "format": build.format, "quant": build.quant, "variant": build.variant, "size_bytes": build.size_bytes,
                            "default_context": context, "max_context": build.max_context,
                            "estimated_memory_bytes": Self::estimate(build, context, sessions),
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
            "port": cfg.port, "share": cfg.share, "key_set": cfg.api_key.as_deref().is_some_and(|k| !k.is_empty()),
            "served_name": cfg.served_name, "memory_mode": cfg.memory_mode, "idle_minutes": cfg.idle_minutes, "auto_switch": cfg.auto_switch,
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
        .route("/8gent/v1/select", post(select))
        .route("/8gent/v1/engine/start", post(engine_start))
        .route("/8gent/v1/engine/stop", post(engine_stop))
        .route("/8gent/v1/rescan", post(rescan))
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
    app.rescan().await;
    Ok(Json(json!({"ok": true, "models": app.library.read().unwrap().models.len()})))
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
    if app.config().memory_mode == MemoryMode::Always && app.config().selection.is_set() {
        let a = app.clone();
        tokio::spawn(async move {
            let sel = a.config().selection;
            if let Err(e) = a.acquire(&sel).await {
                eprintln!("8gent: could not load {} at start: {}", sel.model, normalize::error_message(&e.body).unwrap_or_default());
            }
        });
    }
    let result = axum::serve(listener, router(app.clone()).into_make_service_with_connect_info::<SocketAddr>()).with_graceful_shutdown(shutdown).await;
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
