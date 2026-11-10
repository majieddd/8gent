//! `8gent serve` runs the service; every other subcommand asks a running service over its control API.

use eightgent::config::Config;
use eightgent::paths::Paths;
use eightgent::server::{self, App, Options};
use serde_json::{Value, json};
use std::net::SocketAddr;

const HELP: &str = "8gent: one local endpoint in front of any model loader

  8gent serve [--port N]              run the service (the endpoint is http://127.0.0.1:8436/v1)
  8gent status                        what is selected, loaded and being answered
  8gent loaders                       the loaders, and which are installed here
  8gent models                        the models, and which loaders can run each
  8gent use <model> [--loader L] [--context N] [--thinking on|off] [--no-load]
                                      select a model (and loader); goes back if it will not load
  8gent start | stop                  load or unload the selected model
  8gent logs [--loader L] [-n N]      the engine's own output
  8gent rescan                        look again for loaders and models
  8gent service install | uninstall | status
                                      start the service at login (and now), or stop doing so

  EIGHTGENT_HOME    the folder for config, logs and state (default: the per-user data folder)
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("help");
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("runtime");
    let code = rt.block_on(async {
        match command {
            "serve" => serve(&args[1..]).await,
            "help" | "--help" | "-h" => {
                print!("{HELP}");
                Ok(())
            }
            "--version" | "-V" | "version" => {
                println!("8gent {}", eightgent::VERSION);
                Ok(())
            }
            "service" => service(&args[1..]).await,
            _ => client(command, &args[1..]).await,
        }
    });
    if let Err(message) = code {
        eprintln!("8gent: {message}");
        std::process::exit(1);
    }
}

fn option<'a>(args: &'a [String], names: &[&str]) -> Option<&'a str> {
    args.iter().position(|a| names.contains(&a.as_str())).and_then(|i| args.get(i + 1)).map(String::as_str)
}

async fn serve(args: &[String]) -> Result<(), String> {
    let paths = Paths::from_env();
    let mut opts = Options::new(paths);
    opts.import_external = std::env::var_os("EIGHTGENT_NO_IMPORT").is_none();
    let app = App::build(opts).await?;
    let cfg = app.config();
    let port = option(args, &["--port", "-p"]).and_then(|p| p.parse().ok()).unwrap_or(cfg.port);
    let host = if cfg.share { [0, 0, 0, 0] } else { [127, 0, 0, 1] };
    let addr = SocketAddr::from((host, port));
    // a service that is handing over to this one may hold the port for a few more seconds
    let mut tries = 0;
    let listener = loop {
        match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => break l,
            Err(e) if tries >= 40 => return Err(format!("cannot listen on {addr}: {e} (is 8gent already running?)")),
            Err(_) => {
                tries += 1;
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }
    };
    for alias in cfg.also_listen.iter().filter(|p| **p != port) {
        tokio::spawn(server::serve_alias(app.clone(), SocketAddr::from((host, *alias))));
    }
    eprintln!("8gent {}: serving http://{addr}/v1 ({} loaders; data in {})", eightgent::VERSION, app.manifests.len(), app.paths.home.display());
    server::serve(app, listener, shutdown_signal()).await
}

/// `8gent service install`: from now on the service starts at login, and it is started now.
async fn service(args: &[String]) -> Result<(), String> {
    use eightgent::service;
    let paths = Paths::from_env();
    let port = Config::load(&paths).map(|c| c.port).unwrap_or(eightgent::config::DEFAULT_PORT);
    let health = format!("http://127.0.0.1:{port}/health");
    let http = reqwest::Client::new();
    let up = || async { http.get(&health).timeout(std::time::Duration::from_millis(800)).send().await.is_ok_and(|r| r.status().is_success()) };
    match args.first().map(String::as_str) {
        Some("install") => {
            let program = std::env::current_exe().map_err(|e| e.to_string())?;
            service::write(&program, &paths)?;
            if up().await {
                // one that was started by hand or by the app: it lets go of the port first
                let _ = http.post(format!("http://127.0.0.1:{port}/8gent/v1/shutdown")).header("content-type", "application/json").body("{}").send().await;
                for _ in 0..40 {
                    if !up().await {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                }
            }
            service::load()?;
            for _ in 0..60 {
                if up().await {
                    println!("8gent now starts at login and is running ({})", program.display());
                    return Ok(());
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
            Err(format!("the service was set up but is not answering; see {}", paths.logs().join("service.log").display()))
        }
        Some("uninstall") => {
            service::remove(true)?;
            println!("8gent no longer starts at login, and its service is stopped");
            Ok(())
        }
        Some("status") => {
            println!("starts at login: {}", if service::installed() { "yes" } else { "no" });
            println!("running: {}", if up().await { "yes" } else { "no" });
            Ok(())
        }
        _ => Err("usage: 8gent service install | uninstall | status".into()),
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("signal");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
    eprintln!("8gent: stopping");
}

// ----------------------------------------------------------------------------------------------- client

async fn client(command: &str, args: &[String]) -> Result<(), String> {
    let paths = Paths::from_env();
    let port = std::env::var("EIGHTGENT_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or_else(|| Config::load(&paths).map(|c| c.port).unwrap_or(eightgent::config::DEFAULT_PORT));
    let base = format!("http://127.0.0.1:{port}/8gent/v1");
    let http = reqwest::Client::new();
    let get = |path: &str| {
        let (http, url) = (http.clone(), format!("{base}{path}"));
        async move { call(http.get(url)).await }
    };
    let post = |path: &str, body: Value| {
        let (http, url) = (http.clone(), format!("{base}{path}"));
        async move { call(http.post(url).json(&body)).await }
    };
    match command {
        "status" => print_status(&get("/status").await?),
        "loaders" => print_loaders(&get("/loaders").await?),
        "models" => print_models(&get("/models").await?),
        "start" => {
            let v = post("/engine/start", json!({})).await?;
            println!("loaded {} on {} in {} s", s(&v["running"]["model"]), s(&v["running"]["loader"]), v["running"]["load_seconds"]);
        }
        "stop" => {
            post("/engine/stop", json!({})).await?;
            println!("unloaded");
        }
        "rescan" => {
            let v = post("/rescan", json!({})).await?;
            println!("{} models", v["models"]);
        }
        "logs" => {
            let mut path = format!("/logs?lines={}", option(args, &["-n", "--lines"]).unwrap_or("60"));
            if let Some(l) = option(args, &["--loader", "-l"]) {
                path.push_str(&format!("&loader={l}"));
            }
            let v = get(&path).await?;
            for line in v["lines"].as_array().into_iter().flatten() {
                println!("{}", s(line));
            }
        }
        "use" => {
            let model = args.first().filter(|a| !a.starts_with('-')).ok_or("usage: 8gent use <model> [--loader L] [--context N] [--thinking on|off] [--no-load]")?;
            let mut body = json!({"model": model});
            if let Some(l) = option(args, &["--loader", "-l"]) {
                body["loader"] = json!(l);
            }
            if let Some(c) = option(args, &["--context", "-c"]) {
                body["context"] = json!(parse_size(c).ok_or(format!("\"{c}\" is not a context size (try 65536 or 64K)"))?);
            }
            if let Some(t) = option(args, &["--thinking"]) {
                body["thinking"] = json!(t == "on");
            }
            if args.iter().any(|a| a == "--no-load") {
                body["load"] = json!(false);
            }
            let v = post("/select", body).await?;
            let sel = &v["selection"];
            let loaded = if v["loaded"] == true { format!(", loaded{}", v["load_seconds"].as_f64().map(|t| format!(" in {t} s")).unwrap_or_default()) } else { ", not loaded yet".into() };
            println!("selected {} on {}{loaded}", s(&sel["model"]), s(&sel["loader"]));
        }
        other => return Err(format!("unknown command \"{other}\"\n\n{HELP}")),
    }
    Ok(())
}

async fn call(req: reqwest::RequestBuilder) -> Result<Value, String> {
    let resp = req.send().await.map_err(|e| if e.is_connect() { "the service is not running (start it with `8gent serve`)".to_string() } else { e.to_string() })?;
    let ok = resp.status().is_success();
    let body: Value = resp.json().await.map_err(|e| e.to_string())?;
    if ok {
        return Ok(body);
    }
    // the sentence, then the engine's own last output when the service passed it on
    let message = body["error"]["message"].as_str().unwrap_or("the service refused the request");
    match body["error"]["detail"].as_str().filter(|d| !d.is_empty()) {
        Some(detail) => Err(format!("{message}\n\n{detail}")),
        None => Err(message.to_string()),
    }
}

/// "64K" and "1M" count in 1024s, like the context sizes models are built with.
fn parse_size(text: &str) -> Option<u64> {
    let t = text.trim().replace(',', "").to_uppercase();
    let (number, factor) = match t.chars().last()? {
        'K' => (&t[..t.len() - 1], 1024.0),
        'M' => (&t[..t.len() - 1], 1024.0 * 1024.0),
        _ => (t.as_str(), 1.0),
    };
    number.parse::<f64>().ok().filter(|n| *n > 0.0).map(|n| (n * factor) as u64)
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("-")
}

fn gb(v: &Value) -> String {
    v.as_f64().map(|b| format!("{:.1} GB", b / 1e9)).unwrap_or_else(|| "-".into())
}

fn print_status(v: &Value) {
    let (sel, engine) = (&v["selection"], &v["engine"]);
    println!("8gent {} on {} ({} {})", s(&v["version"]), format!("http://127.0.0.1:{}/v1", v["port"]), s(&v["platform"]), s(&v["os_version"]));
    if sel["model"].as_str().is_some_and(|m| !m.is_empty()) {
        println!("selected   {} on {}", s(&sel["model"]), s(&sel["loader"]));
    } else {
        println!("selected   nothing yet (8gent use <model>)");
    }
    let state = s(&engine["state"]);
    match state {
        "ready" => {
            let r = &engine["running"];
            println!("engine     ready: {} on {} {}, context {}, {} in memory, loaded in {} s", s(&r["model"]), s(&r["loader"]), s(&r["loader_version"]), r["context"], gb(&engine["memory_bytes"]), r["load_seconds"]);
        }
        "loading" => println!("engine     loading {} on {} ({} s so far)", s(&engine["loading"]["model"]), s(&engine["loading"]["loader"]), engine["loading"]["seconds"]),
        "error" => println!("engine     problem: {}", s(&engine["error"])),
        _ => println!("engine     not loaded (the next request loads it)"),
    }
    let t = &v["stats"]["totals"];
    println!("answered   {} requests ({} errors), {} tokens read, {} written; {} in progress", t["requests"], t["errors"], t["prompt_tokens"], t["completion_tokens"], v["stats"]["in_flight"]);
    let o = &v["stats"]["overflows"];
    if o["count"].as_u64().unwrap_or(0) > 0 {
        println!("too long   {} prompts did not fit the {}-token window (largest {} tokens)", o["count"], o["context"], o["largest_tokens"]);
    }
    for sp in v["stats"]["speed"].as_array().into_iter().flatten() {
        if let Some(tps) = sp["mean_tokens_per_second"].as_f64() {
            println!("speed      {} on {}: {tps:.1} tokens/s over {} requests", s(&sp["model"]), s(&sp["loader"]), sp["requests"]);
        }
    }
}

fn print_loaders(v: &Value) {
    for l in v["loaders"].as_array().into_iter().flatten() {
        let state = if l["supported_here"] == false {
            "not for this computer".to_string()
        } else if l["installed"] == true {
            match l["kind"].as_str() {
                Some("attach") => format!("server answering{}", l["version"].as_str().map(|x| format!(", {x}")).unwrap_or_default()),
                _ => format!("installed{}", l["version"].as_str().map(|x| format!(", {x}")).unwrap_or_default()),
            }
        } else if l["kind"] == "attach" {
            if l["program"].is_null() { "not installed".into() } else { "installed, its server is not running".into() }
        } else {
            "not installed".into()
        };
        let mark = if l["selected"] == true { "*" } else { " " };
        println!("{mark} {:<11} {:<12} {state}; runs {}; {} models", s(&l["id"]), s(&l["name"]), l["formats"].as_array().map(|f| f.iter().map(s).collect::<Vec<_>>().join(", ")).unwrap_or_default(), l["models"].as_array().map_or(0, |m| m.len()));
        for r in l["requirements"].as_array().into_iter().flatten() {
            if r["met"] == false {
                let scope = r["since"].as_str().map(|x| format!(" from {x}")).unwrap_or_default();
                println!("              needs {} {}{scope}: {}", s(&r["os"]), s(&r["min"]), s(&r["note"]));
            }
        }
    }
}

fn print_models(v: &Value) {
    for m in v["models"].as_array().into_iter().flatten() {
        let mark = if m["selected"] == true { "*" } else { " " };
        println!("{mark} {}  ({})", s(&m["id"]), s(&m["name"]));
        for l in m["loaders"].as_array().into_iter().flatten() {
            let detail = [l["quant"].as_str().map(String::from), l["size_bytes"].as_f64().map(|b| format!("{:.1} GB", b / 1e9)), l["default_context"].as_u64().map(|c| format!("context {c}")), l["estimated_memory_bytes"].as_f64().map(|b| format!("about {:.1} GB loaded", b / 1e9))];
            let detail: Vec<String> = detail.into_iter().flatten().collect();
            let able = if l["runnable"] == true { String::new() } else { format!("  [cannot run: {}]", s(&l["why_not"])) };
            println!("      {:<11} {} {}{able}", s(&l["id"]), s(&l["format"]), detail.join(", "));
        }
    }
}
