//! What every loader must look like through 8gent, tested against the stand-in engine (no model, no
//! GPU): each test starts its own service on a spare port with its own data folder.

use eightgent::paths::Paths;
use eightgent::server::{self, App, Options};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

static NEXT: AtomicU32 = AtomicU32::new(0);

struct Service {
    base: String,
    home: PathBuf,
    http: reqwest::Client,
    _app: Arc<App>,
}

impl Drop for Service {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

/// Two loaders that both run the "mock" format: `mocka` behaves like a plain OpenAI server, `mockb`
/// like uzu (strict roles, errors inside 200 answers, compact stream lines); `mockfail` never loads.
fn loader(id: &str, extra_args: &str, api: &str, formats: &str) -> String {
    let engine = env!("CARGO_BIN_EXE_mock-engine");
    format!(
        r#"id = "{id}"
name = "{id}"
kind = "managed"
formats = [{formats}]
[detect]
paths = ['{engine}']
[start]
args = ["--port", "{{port}}", "--name", "{id}", "--model-id", "{{model_id}}"{extra_args}]
ready_path = "/health"
ready_timeout_s = 20
[api]
{api}
"#
    )
}

const MODELS: &str = r#"
[[model]]
id = "alpha"
name = "Alpha"
[[model.build]]
format = "mock"
ref = "alpha-weights"
max_context = 131072
size_bytes = 50000000
kv_bytes_per_token = 1024

[[model]]
id = "beta"
name = "Beta"
[[model.build]]
format = "mock"
ref = "beta-weights"
max_context = 32768

[[model]]
id = "huge"
name = "Huge"
[[model.build]]
format = "mock"
ref = "huge-weights"
size_bytes = 900000000000
kv_bytes_per_token = 32768

[[model]]
id = "gamma"
name = "Gamma, only as another format"
[[model.build]]
format = "other"
ref = "gamma-weights"
"#;

async fn service_with(config: &str) -> Service {
    service_started(config, true).await
}

/// `scanned`: the model list is read before the service starts (false: in the background, as the
/// real service does).
async fn service_started(config: &str, scanned: bool) -> Service {
    let home = std::env::temp_dir().join(format!("8gent-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join("loaders")).unwrap();
    std::fs::write(home.join("loaders/mocka.toml"), loader("mocka", "", "thinking = \"enable_thinking\"", "\"mock\"")).unwrap();
    std::fs::write(home.join("loaders/mockb.toml"), loader("mockb", ", \"--strict-roles\", \"--overflow\", \"uzu\", \"--compact-sse\"", "role_normalize = true", "\"mock\"")).unwrap();
    std::fs::write(home.join("loaders/mockfail.toml"), loader("mockfail", ", \"--fail\"", "", "\"mock\"")).unwrap();
    std::fs::write(home.join("models.toml"), MODELS).unwrap();
    std::fs::write(home.join("config.toml"), config).unwrap();
    let mut opts = Options::new(Paths::at(&home));
    opts.import_external = false;
    opts.wait_for_scan = scanned;
    opts.housekeeping_every = Duration::from_millis(200);
    let app = App::build(opts).await.unwrap();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let served = app.clone();
    tokio::spawn(async move { server::serve(served, listener, std::future::pending()).await });
    Service { base, home, http: reqwest::Client::new(), _app: app }
}

async fn service() -> Service {
    service_with("[selection]\nloader = \"mocka\"\nmodel = \"alpha\"\n").await
}

impl Service {
    async fn get(&self, path: &str) -> (u16, Value) {
        let r = self.http.get(format!("{}{path}", self.base)).send().await.unwrap();
        (r.status().as_u16(), r.json().await.unwrap_or(Value::Null))
    }
    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        let r = self.http.post(format!("{}{path}", self.base)).json(&body).send().await.unwrap();
        (r.status().as_u16(), r.json().await.unwrap_or(Value::Null))
    }
    async fn chat(&self, model: &str, content: &str) -> (u16, Value) {
        self.post("/v1/chat/completions", json!({"model": model, "messages": [{"role": "user", "content": content}]})).await
    }
    /// A streamed chat: status, the whole body as text, and the time until the headers came.
    async fn stream(&self, body: Value) -> (u16, String, Duration) {
        let began = Instant::now();
        let r = self.http.post(format!("{}/v1/chat/completions", self.base)).json(&body).send().await.unwrap();
        let headers_after = began.elapsed();
        (r.status().as_u16(), r.text().await.unwrap(), headers_after)
    }
    async fn status(&self) -> Value {
        self.get("/8gent/v1/status").await.1
    }
    /// The last request body the running stand-in engine received.
    async fn engine_saw(&self) -> Value {
        let base = self.status().await["engine"]["running"]["base_url"].as_str().unwrap().to_string();
        self.http.get(format!("{base}/mock/last")).send().await.unwrap().json().await.unwrap()
    }
}

fn reply(v: &Value) -> &str {
    v["choices"][0]["message"]["content"].as_str().unwrap_or("")
}

#[tokio::test]
async fn model_list_names_what_can_run_and_does_not_load_anything() {
    let s = service().await;
    let (code, v) = s.get("/v1/models").await;
    assert_eq!(code, 200);
    let data = v["data"].as_array().unwrap();
    assert_eq!(data[0]["id"], "8gent");
    assert_eq!(data[0]["context_length"], 65536, "the default window, since Alpha allows more");
    assert_eq!(data[0]["loader"], "mocka");
    let ids: Vec<&str> = data.iter().map(|m| m["id"].as_str().unwrap()).collect();
    for pair in ["mocka/alpha", "mockb/alpha", "mocka/beta"] {
        assert!(ids.contains(&pair), "{pair} in {ids:?}");
    }
    assert!(!ids.iter().any(|i| i.ends_with("/gamma")), "no loader runs Gamma's format");
    let beta = data.iter().find(|m| m["id"] == "mocka/beta").unwrap();
    assert_eq!(beta["context_length"], 32768, "a model's own maximum caps the default");
    assert_eq!(s.status().await["engine"]["state"], "stopped");
}

#[tokio::test]
async fn a_read_does_not_wake_the_model() {
    let s = service().await;
    let (code, v) = s.get("/v1/props").await;
    assert_eq!(code, 503);
    assert_eq!(v["error"]["code"], "engine_unavailable");
    assert_eq!(s.status().await["engine"]["state"], "stopped");
    assert_eq!(s.get("/nothing-here").await.0, 404);
}

#[tokio::test]
async fn a_chat_loads_the_model_and_answers_under_the_name_asked_for() {
    let s = service().await;
    let (code, v) = s.chat("8gent", "hello").await;
    assert_eq!(code, 200, "{v}");
    assert_eq!(reply(&v), "reply from mocka");
    assert_eq!(v["model"], "8gent");
    let st = s.status().await;
    assert_eq!(st["engine"]["state"], "ready");
    assert_eq!(st["engine"]["running"]["context"], 65536);
    let saw = s.engine_saw().await;
    assert_eq!(saw["model"], "alpha", "the engine gets its own model name");
    assert_eq!(saw["enable_thinking"], false, "the selected thinking mode is applied when the request does not choose");
    // any name a client is configured with still reaches the selection
    let (code, v) = s.chat("some-name-hermes-was-given", "hello").await;
    assert_eq!((code, v["model"].as_str().unwrap()), (200, "some-name-hermes-was-given"));
    // a request that chooses its own thinking mode keeps it
    s.post("/v1/chat/completions", json!({"model": "8gent", "enable_thinking": true, "messages": [{"role": "user", "content": "x"}]})).await;
    assert_eq!(s.engine_saw().await["enable_thinking"], true);
    assert_eq!(s.status().await["stats"]["totals"]["requests"], 3);
}

#[tokio::test]
async fn a_stream_passes_through_with_the_usage_event_only_when_asked() {
    let s = service().await;
    let (code, text, _) = s.stream(json!({"model": "8gent", "stream": true, "messages": [{"role": "user", "content": "hi"}]})).await;
    assert_eq!(code, 200);
    assert!(text.contains("\"content\":\"reply\"") && text.trim_end().ends_with("data: [DONE]"), "{text}");
    assert!(text.contains("\"model\":\"8gent\"") && !text.contains("\"model\":\"alpha\""), "{text}");
    assert!(!text.contains("\"usage\""), "the client did not ask for usage: {text}");
    let recent = &s.status().await["stats"]["recent"][0];
    assert_eq!((recent["finish"].as_str(), recent["prompt_tokens"].as_u64(), recent["completion_tokens"].as_u64()), (Some("ok"), Some(12), Some(3)), "{recent}");
    assert!(recent["first_token_ms"].is_u64());

    let (_, text, _) = s.stream(json!({"model": "8gent", "stream": true, "stream_options": {"include_usage": true}, "messages": [{"role": "user", "content": "hi"}]})).await;
    assert!(text.contains("\"usage\""), "{text}");
}

#[tokio::test]
async fn a_history_that_resumes_with_the_assistant_is_repaired_for_a_strict_loader() {
    let s = service_with("[selection]\nloader = \"mockb\"\nmodel = \"alpha\"\n").await;
    let history = json!([{"role": "system", "content": "s"}, {"role": "assistant", "content": "earlier"}, {"role": "user", "content": "go on"}]);
    let (code, v) = s.post("/v1/chat/completions", json!({"model": "8gent", "messages": history})).await;
    assert_eq!(code, 200, "{v}");
    assert_eq!(reply(&v), "reply from mockb");
    assert_eq!(s.engine_saw().await["messages"].as_array().unwrap().len(), 4);
    // a loader that does not need it gets the history as sent
    let (code, _) = s.post("/v1/chat/completions", json!({"model": "mocka/alpha", "messages": history})).await;
    assert_eq!(code, 200);
    assert_eq!(s.engine_saw().await["messages"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn a_prompt_that_does_not_fit_gets_the_standard_error_from_every_loader() {
    let s = service().await;
    // an engine that already answers the standard way: its exact count is kept
    let (code, v) = s.chat("8gent", "OVERFLOW").await;
    assert_eq!((code, v["error"]["code"].as_str()), (400, Some("context_length_exceeded")), "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("resulted in 70123 tokens"), "{v}");
    // an engine that reports it inside a 200 answer, plain and streamed
    let (code, v) = s.chat("mockb/alpha", "OVERFLOW").await;
    assert_eq!((code, v["error"]["code"].as_str()), (400, Some("context_length_exceeded")), "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("maximum context length is 65536"), "{v}");
    let (code, text, _) = s.stream(json!({"model": "mockb/alpha", "stream": true, "messages": [{"role": "user", "content": "OVERFLOW"}]})).await;
    assert_eq!(code, 400, "{text}");
    assert!(text.contains("context_length_exceeded") && !text.starts_with("data:"), "{text}");
    let st = s.status().await;
    assert_eq!(st["stats"]["overflows"]["count"], 3);
    assert_eq!(st["stats"]["recent"][0]["finish"], "overflow");
}

#[tokio::test]
async fn naming_a_loader_in_a_request_switches_to_it() {
    let s = service().await;
    assert_eq!(reply(&s.chat("8gent", "hi").await.1), "reply from mocka");
    let (code, v) = s.chat("mockb/alpha", "hi").await;
    assert_eq!((code, reply(&v), v["model"].as_str()), (200, "reply from mockb", Some("mockb/alpha")));
    let st = s.status().await;
    assert_eq!((st["selection"]["loader"].as_str(), st["engine"]["running"]["loader"].as_str()), (Some("mockb"), Some("mockb")));
    assert_eq!(reply(&s.chat("8gent", "hi").await.1), "reply from mockb", "the served name follows the switch");
    // a model id alone keeps the loader that is selected
    let (code, v) = s.chat("beta", "hi").await;
    assert_eq!((code, reply(&v)), (200, "reply from mockb"));
    assert_eq!(s.status().await["engine"]["running"]["model"], "beta");
    assert_eq!(s.status().await["engine"]["running"]["context"], 32768);
}

#[tokio::test]
async fn switching_on_request_can_be_turned_off() {
    let s = service_with("auto_switch = false\n[selection]\nloader = \"mocka\"\nmodel = \"alpha\"\n").await;
    let (code, v) = s.chat("mockb/alpha", "hi").await;
    assert_eq!((code, v["error"]["code"].as_str()), (409, Some("model_not_selected")));
    assert_eq!(reply(&s.chat("8gent", "hi").await.1), "reply from mocka");
}

#[tokio::test]
async fn a_small_model_stays_loaded_next_to_the_selected_one() {
    let s = service().await;
    // nothing chosen: its name says so
    let (code, v) = s.chat("8gent-small", "hi").await;
    assert_eq!(code, 503, "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("No small always-on model"));
    // choose one: it loads at once, on its own engine
    let (code, v) = s.post("/8gent/v1/small", json!({"model": "beta", "loader": "mockb"})).await;
    assert_eq!((code, v["loaded"].as_bool()), (200, Some(true)), "{v}");
    assert_eq!(reply(&s.chat("8gent", "hi").await.1), "reply from mocka");
    let (code, v) = s.chat("8gent-small", "hi").await;
    assert_eq!((code, reply(&v), v["model"].as_str()), (200, "reply from mockb", Some("8gent-small")));
    let st = s.status().await;
    assert_eq!((st["engine"]["running"]["model"].as_str(), st["small"]["running"]["model"].as_str()), (Some("alpha"), Some("beta")), "both are loaded");
    assert_eq!(st["small"]["running"]["context"], 8192, "a small window unless one is named");
    assert_ne!(st["engine"]["running"]["pid"], st["small"]["running"]["pid"]);
    let ids: Vec<String> = s.get("/v1/models").await.1["data"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap().to_string()).collect();
    assert!(ids.contains(&"8gent-small".to_string()), "{ids:?}");
    // switching the selected model leaves the small one alone
    s.chat("mockb/alpha", "hi").await;
    assert_eq!(s.status().await["small"]["state"], "ready");
    // one that will not load: the choice goes back, and the old one is serving again
    let (code, _) = s.post("/8gent/v1/small", json!({"model": "alpha", "loader": "mockfail"})).await;
    assert_eq!(code, 503);
    assert_eq!(s.status().await["small"]["selection"]["model"], "beta");
    assert_eq!(reply(&s.chat("8gent-small", "hi").await.1), "reply from mockb");
    // off
    let (code, v) = s.post("/8gent/v1/small", json!({"off": true})).await;
    assert_eq!((code, v["loaded"].as_bool()), (200, Some(false)));
    assert_eq!(s.status().await["small"]["state"], "stopped");
    assert_eq!(s.chat("8gent-small", "hi").await.0, 503);
}

#[tokio::test]
async fn a_load_that_cannot_fit_is_refused_before_anything_is_unloaded() {
    let s = service().await;
    assert_eq!(reply(&s.chat("8gent", "hi").await.1), "reply from mocka");
    let knows_the_limit = s.status().await["memory"]["gpu_budget_bytes"].is_u64();
    let (code, v) = s.post("/8gent/v1/select", json!({"model": "huge", "load": true})).await;
    if knows_the_limit {
        assert_eq!((code, v["error"]["code"].as_str(), v["can_force"].as_bool()), (409, Some("not_enough_memory"), Some(true)), "{v}");
        assert!(v["error"]["message"].as_str().unwrap().contains("Huge at a 65536-token window needs about 90"), "{v}");
        let st = s.status().await;
        assert_eq!((st["selection"]["model"].as_str(), st["engine"]["state"].as_str()), (Some("alpha"), Some("ready")), "what was serving is untouched");
        // the person can insist
        let (code, v) = s.post("/8gent/v1/select", json!({"model": "huge", "load": true, "force": true})).await;
        assert_eq!((code, v["loaded"].as_bool()), (200, Some(true)), "{v}");
    } else {
        // a computer whose limit 8gent does not know yet: nothing is refused
        assert_eq!(code, 200, "{v}");
    }
}

#[tokio::test]
async fn a_switch_that_will_not_load_goes_back_to_what_was_running() {
    let s = service().await;
    assert_eq!(reply(&s.chat("8gent", "hi").await.1), "reply from mocka");
    let (code, v) = s.post("/8gent/v1/select", json!({"loader": "mockfail"})).await;
    assert_eq!(code, 409, "{v}");
    assert_eq!((v["rolled_back"].as_bool(), v["restored"].as_bool()), (Some(true), Some(true)), "{v}");
    let msg = v["error"]["message"].as_str().unwrap();
    assert!(msg.contains("mockfail stopped while loading Alpha") && msg.ends_with("8gent went back to Alpha on mocka."), "{msg}");
    assert!(v["error"]["detail"].as_str().unwrap().contains("not enough memory"), "the engine's own reason is passed on: {v}");
    let st = s.status().await;
    assert_eq!((st["selection"]["loader"].as_str(), st["engine"]["state"].as_str()), (Some("mocka"), Some("ready")));
    assert_eq!(reply(&s.chat("8gent", "hi").await.1), "reply from mocka");
    // the saved config still names the old selection
    assert!(std::fs::read_to_string(s.home.join("config.toml")).unwrap().contains("loader = \"mocka\""));
}

#[tokio::test]
async fn selecting_changes_loader_model_and_window() {
    let s = service().await;
    // nothing loaded yet: selecting does not load
    let (code, v) = s.post("/8gent/v1/select", json!({"model": "beta"})).await;
    assert_eq!((code, v["loaded"].as_bool()), (200, Some(false)), "{v}");
    assert_eq!(s.status().await["engine"]["state"], "stopped");
    // asked to load: it does, on the loader that was selected
    let (code, v) = s.post("/8gent/v1/select", json!({"model": "alpha", "context": 131072, "load": true})).await;
    assert_eq!((code, v["loaded"].as_bool()), (200, Some(true)), "{v}");
    let run = s.status().await["engine"]["running"].clone();
    assert_eq!((run["loader"].as_str(), run["model"].as_str(), run["context"].as_u64()), (Some("mocka"), Some("alpha"), Some(131072)));
    // once loaded, a change of loader reloads without being asked
    let (code, v) = s.post("/8gent/v1/select", json!({"loader": "mockb"})).await;
    assert_eq!((code, v["restarted"].as_bool()), (200, Some(true)), "{v}");
    assert_eq!(s.status().await["engine"]["running"]["context"], 131072, "the window is kept when only the loader changes");
    // a model's window is remembered: coming back to Alpha by name brings its 131,072 back
    s.chat("mockb/beta", "hi").await;
    assert_eq!(s.status().await["engine"]["running"]["context"], 32768);
    s.chat("mockb/alpha", "hi").await;
    assert_eq!(s.status().await["engine"]["running"]["context"], 131072);
    // refusals change nothing
    for (body, part) in [
        (json!({"model": "nope"}), "no model called"),
        (json!({"model": "gamma"}), "No loader installed here can run"),
        (json!({"loader": "mocka", "model": "gamma"}), "cannot run"),
        (json!({"model": "beta", "context": 65536}), "at most 32768"),
        (json!({"loader": "ghost"}), "no loader called"),
    ] {
        let (code, v) = s.post("/8gent/v1/select", body.clone()).await;
        assert_eq!(code, 400, "{body} -> {v}");
        assert!(v["error"]["message"].as_str().unwrap().contains(part), "{body} -> {v}");
    }
    assert_eq!(s.status().await["selection"]["loader"], "mockb");
}

#[tokio::test]
async fn a_slow_prompt_read_still_gets_its_headers_early() {
    let s = service().await;
    s.chat("8gent", "warm up").await;
    let (code, text, headers_after) = s.stream(json!({"model": "8gent", "stream": true, "messages": [{"role": "user", "content": "PREFILL3"}]})).await;
    assert_eq!(code, 200);
    assert!(headers_after < Duration::from_millis(2700), "headers after {headers_after:?}");
    assert!(text.contains("\"content\":\"reply\"") && text.contains("[DONE]"));
}

#[tokio::test]
async fn an_idle_model_is_unloaded_and_the_next_request_loads_it_again() {
    let s = service_with("idle_minutes = 0.02\n[selection]\nloader = \"mocka\"\nmodel = \"alpha\"\n").await;
    s.chat("8gent", "hi").await;
    assert_eq!(s.status().await["engine"]["state"], "ready");
    let deadline = Instant::now() + Duration::from_secs(8);
    while s.status().await["engine"]["state"] != "stopped" {
        assert!(Instant::now() < deadline, "still loaded after 8 s");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(reply(&s.chat("8gent", "hi").await.1), "reply from mocka");
}

#[tokio::test]
async fn a_model_kept_loaded_is_loaded_when_the_service_starts() {
    // the model list is still being read when the service starts: the load waits for it
    let s = service_started("memory_mode = \"always\"\n[selection]\nloader = \"mocka\"\nmodel = \"alpha\"\n", false).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    while s.status().await["engine"]["state"] != "ready" {
        assert!(Instant::now() < deadline, "not loaded 10 s after starting: {}", s.status().await["engine"]);
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(s.status().await["engine"]["running"]["model"], "alpha");
}

#[tokio::test]
async fn keep_loaded_mode_does_not_unload() {
    let s = service_with("idle_minutes = 0.02\nmemory_mode = \"on_demand\"\n[selection]\nloader = \"mocka\"\nmodel = \"alpha\"\n").await;
    s.chat("8gent", "hi").await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(s.status().await["engine"]["state"], "ready");
}

#[cfg(unix)]
#[tokio::test]
async fn an_engine_that_dies_is_noticed_and_started_again_on_the_next_request() {
    let s = service().await;
    s.chat("8gent", "hi").await;
    let pid = s.status().await["engine"]["running"]["pid"].as_u64().unwrap();
    std::process::Command::new("kill").args(["-9", &pid.to_string()]).status().unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let st = s.status().await;
    assert_eq!(st["engine"]["state"], "error", "{st}");
    assert!(st["engine"]["error"].as_str().unwrap().contains("stopped by itself"));
    let (code, v) = s.chat("8gent", "hi").await;
    assert_eq!((code, reply(&v)), (200, "reply from mocka"));
    assert_ne!(s.status().await["engine"]["running"]["pid"].as_u64().unwrap(), pid);
}

#[tokio::test]
async fn stop_and_start_and_the_lists() {
    let s = service().await;
    let (code, v) = s.post("/8gent/v1/engine/start", json!({})).await;
    assert_eq!((code, v["running"]["model"].as_str()), (200, Some("alpha")), "{v}");
    assert_eq!(s.post("/8gent/v1/engine/stop", json!({})).await.0, 200);
    assert_eq!(s.status().await["engine"]["state"], "stopped");

    let loaders = s.get("/8gent/v1/loaders").await.1;
    let find = |id: &str| loaders["loaders"].as_array().unwrap().iter().find(|l| l["id"] == id).cloned().unwrap();
    assert_eq!((find("mocka")["installed"].as_bool(), find("mocka")["selected"].as_bool()), (Some(true), Some(true)));
    assert!(find("mocka")["models"].as_array().unwrap().iter().any(|m| m == "alpha"));
    assert!(loaders["loaders"].as_array().unwrap().iter().any(|l| l["id"] == "llamacpp"), "the built-in loaders are listed too");

    let models = s.get("/8gent/v1/models").await.1;
    let alpha = models["models"].as_array().unwrap().iter().find(|m| m["id"] == "alpha").cloned().unwrap();
    let on_a = alpha["loaders"].as_array().unwrap().iter().find(|l| l["id"] == "mocka").cloned().unwrap();
    assert_eq!(on_a["runnable"], true);
    // weights 50 MB + 1.1 GB runtime + 1 KB a token over 65,536 tokens (small enough for any test machine)
    assert_eq!(on_a["estimated_memory_bytes"].as_u64(), Some(50_000_000 + 1_100_000_000 + 1024 * 65536));
    assert_eq!((on_a["base_bytes"].as_u64(), on_a["kv_bytes_per_token"].as_u64(), on_a["context"].as_u64()), (Some(1_150_000_000), Some(1024), Some(65536)));
    assert_eq!((on_a["selected"].as_bool(), on_a["serving"].as_bool()), (Some(true), Some(false)));

    // settings are saved and answered back; nonsense is refused
    let (code, v) = s.post("/8gent/v1/settings", json!({"memory_mode": "on_demand", "idle_minutes": 10, "auto_switch": false})).await;
    assert_eq!((code, v["memory_mode"].as_str(), v["idle_minutes"].as_f64(), v["auto_switch"].as_bool()), (200, Some("on_demand"), Some(10.0), Some(false)), "{v}");
    // updates: a loader its own app updates is refused; an unknown one too; the list answers
    assert_eq!(s.post("/8gent/v1/loaders/ollama/update", json!({})).await.0, 400);
    assert_eq!(s.post("/8gent/v1/loaders/ghost/update", json!({})).await.0, 400);
    assert_eq!(s.get("/8gent/v1/updates").await.1["auto_update"], true);
    assert_eq!(s.status().await["memory_mode"], "on_demand");
    assert!(std::fs::read_to_string(s.home.join("config.toml")).unwrap().contains("memory_mode = \"on_demand\""));
    assert_eq!(s.post("/8gent/v1/settings", json!({"memory_mode": "sometimes"})).await.0, 400);
    assert_eq!(s.status().await["selection"]["loader"], "mocka", "settings leave the selection alone");

    // the Chat screen's conversations: saved, listed newest first, read back, removed
    let put = |id: &str, body: Value| { let (http, url) = (s.http.clone(), format!("{}/8gent/v1/chats/{id}", s.base)); async move { http.put(url).json(&body).send().await.unwrap().status().as_u16() } };
    assert_eq!(put("first-chat", json!({"title": "About the sea", "messages": [{"role": "user", "content": "hi"}]})).await, 200);
    assert_eq!(put("second-chat", json!({"messages": []})).await, 200);
    // an id cannot leave the chats folder: a slash (sent encoded) or a dot is refused
    assert_eq!(put("..%2F..%2Fconfig", json!({"messages": []})).await, 400);
    assert_eq!(put("bad.id", json!({"messages": []})).await, 400);
    assert_eq!(put("no-messages", json!({"title": "x"})).await, 400);
    let listed = s.get("/8gent/v1/chats").await.1;
    assert_eq!(listed["chats"].as_array().unwrap().len(), 2, "{listed}");
    let one = s.get("/8gent/v1/chats/first-chat").await.1;
    assert_eq!((one["title"].as_str(), one["messages"][0]["content"].as_str()), (Some("About the sea"), Some("hi")));
    assert_eq!(s.http.delete(format!("{}/8gent/v1/chats/first-chat", s.base)).send().await.unwrap().status().as_u16(), 200);
    assert_eq!(s.get("/8gent/v1/chats/first-chat").await.0, 404);

    s.chat("8gent", "hi").await;
    tokio::time::sleep(Duration::from_millis(1300)).await;
    let mon = s.get("/8gent/v1/monitor").await.1;
    let samples = mon["samples"].as_array().unwrap();
    assert!(!samples.is_empty() && samples.iter().any(|x| x["tokens_per_second"].as_f64().unwrap_or(0.0) > 0.0), "the answer's tokens show in the monitor: {mon}");
    assert_eq!(samples.last().unwrap()["state"], "ready");
    let recent = s.get("/8gent/v1/requests?limit=5").await.1;
    assert_eq!((recent["requests"].as_array().unwrap().len(), recent["requests"][0]["finish"].as_str()), (1, Some("ok")), "{recent}");
    let logs = s.get("/8gent/v1/logs?loader=mocka&lines=20").await.1;
    assert!(logs["lines"].as_array().unwrap().iter().any(|l| l.as_str().unwrap().contains("mock-engine: mocka serving alpha")), "{logs}");
}

#[tokio::test]
async fn a_harness_is_pointed_here_and_taken_off_again_with_a_copy_kept() {
    let s = service_with("[selection]\nloader = \"mocka\"\nmodel = \"alpha\"\ncontext = 8192\n").await;
    // a harness described by a file in the data folder, with its settings in the test's own folder
    let settings = s.home.join("their-settings").join("opencode.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    std::fs::write(&settings, "{\n  \"theme\": \"mine\",\n  \"mcp\": {}\n}\n").unwrap();
    std::fs::create_dir_all(s.home.join("harnesses")).unwrap();
    std::fs::write(
        s.home.join("harnesses/scratch.toml"),
        format!("id = \"scratch\"\nname = \"Scratch\"\n[detect]\nprograms = [\"{}\"]\n[connect]\nkind = \"opencode-json\"\nfile = \"{}\"\nuse = \"scratch run\"\n", std::env::current_exe().unwrap().display(), settings.display()),
    )
    .unwrap();
    let find = |v: &Value, id: &str| v["harnesses"].as_array().unwrap().iter().find(|h| h["id"] == id).cloned().unwrap_or(Value::Null);

    let (code, list) = s.get("/8gent/v1/harnesses").await;
    assert_eq!(code, 200, "{list}");
    assert_eq!(list["endpoint"], "http://127.0.0.1:8436/v1");
    for id in ["hermes", "opencode", "claude-code", "kr8"] {
        assert_eq!(find(&list, id)["id"], id, "the built-in harnesses are listed");
    }
    let scratch = find(&list, "scratch");
    assert_eq!((scratch["installed"].as_bool(), scratch["link"]["state"].as_str(), scratch["can_connect"].as_bool()), (Some(true), Some("not_connected"), Some(true)), "{scratch}");

    let (code, done) = s.post("/8gent/v1/harnesses/scratch/connect", json!({})).await;
    assert_eq!(code, 200, "{done}");
    assert_eq!(done["written"]["changed"], true);
    assert_eq!(done["harness"]["link"], json!({"state": "connected", "entries": ["8gent"]}));
    let copy = done["written"]["backup"].as_str().expect("where the copy is");
    assert_eq!(std::fs::read_to_string(copy).unwrap(), "{\n  \"theme\": \"mine\",\n  \"mcp\": {}\n}\n");
    let now: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(now["theme"], "mine");
    assert_eq!(now["provider"]["8gent"]["options"]["baseURL"], "http://127.0.0.1:8436/v1");
    // the window of the model that is selected goes with it
    assert_eq!(now["provider"]["8gent"]["models"]["8gent"]["limit"], json!({"context": 8192, "output": 2048}));

    // again: nothing to do, nothing written, no second copy
    let (code, again) = s.post("/8gent/v1/harnesses/scratch/connect", json!({})).await;
    assert_eq!((code, again["written"]["changed"].as_bool(), again["written"]["backup"].is_null()), (200, Some(false), true), "{again}");

    let (code, off) = s.post("/8gent/v1/harnesses/scratch/disconnect", json!({})).await;
    assert_eq!((code, off["written"]["changed"].as_bool()), (200, Some(true)), "{off}");
    assert_eq!(off["harness"]["link"]["state"], "not_connected");
    let back: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(back, json!({"theme": "mine", "mcp": {}}));

    // settings 8gent cannot read are refused and left alone
    std::fs::write(&settings, "{ // mine\n}\n").unwrap();
    let (code, refused) = s.post("/8gent/v1/harnesses/scratch/connect", json!({})).await;
    assert_eq!(code, 409, "{refused}");
    assert!(refused["error"]["message"].as_str().unwrap().contains("Nothing was changed"), "{refused}");
    assert_eq!(std::fs::read_to_string(&settings).unwrap(), "{ // mine\n}\n");
    assert_eq!(find(&s.get("/8gent/v1/harnesses").await.1, "scratch")["link"]["state"], "unreadable");

    // a harness 8gent does not write for, and one that does not exist
    assert_eq!(s.post("/8gent/v1/harnesses/claude-code/connect", json!({})).await.0, 409);
    let (code, missing) = s.post("/8gent/v1/harnesses/nothing/connect", json!({})).await;
    assert_eq!((code, missing["error"]["code"].as_str()), (404, Some("harness_not_found")), "{missing}");
}
