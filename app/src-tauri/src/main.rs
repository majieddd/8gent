//! The 8gent window. It is a client of the 8gent service and holds no state of its own: the web
//! page inside asks the service's control API through the `api` command, and `start_service`
//! starts the service when it is not running. The service keeps running when the window closes,
//! so the endpoint stays up for Hermes, kr8 and everything else.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eightgent::config::{Config, DEFAULT_PORT};
use eightgent::paths::Paths;
use std::path::PathBuf;
use std::time::Duration;
use tauri::Manager;

#[derive(serde::Serialize)]
struct ApiResponse {
    status: u16,
    body: String,
}

fn port() -> u16 {
    std::env::var("EIGHTGENT_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or_else(|| Config::load(&Paths::from_env()).map(|c| c.port).unwrap_or(DEFAULT_PORT))
}

/// One call to the service's control API. Only `/8gent/v1/...` on this computer can be reached.
#[tauri::command]
async fn api(client: tauri::State<'_, reqwest::Client>, method: String, path: String, body: Option<String>) -> Result<ApiResponse, String> {
    if !path.starts_with("/8gent/v1/") {
        return Err("the window only talks to the control API".into());
    }
    let url = format!("http://127.0.0.1:{}{path}", port());
    let request = match method.as_str() {
        "GET" => client.get(url),
        "POST" => client.post(url).header("content-type", "application/json").body(body.unwrap_or_else(|| "{}".into())),
        "PUT" => client.put(url).header("content-type", "application/json").body(body.unwrap_or_else(|| "{}".into())),
        "DELETE" => client.delete(url),
        other => return Err(format!("{other} is not used by the control API")),
    };
    let response = request.send().await.map_err(|e| e.to_string())?;
    Ok(ApiResponse { status: response.status().as_u16(), body: response.text().await.map_err(|e| e.to_string())? })
}

/// Chats in progress, by the id the page gave them, so the page can stop one.
#[derive(Default)]
struct Streams(std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Notify>>>);

/// One streamed chat request to the endpoint. The answer's text is handed to the page piece by
/// piece as it arrives; the result is the HTTP status. Stopping drops the connection, which the
/// service records as cancelled and the engine stops writing.
#[tauri::command]
async fn chat_stream(client: tauri::State<'_, reqwest::Client>, streams: tauri::State<'_, Streams>, id: String, body: String, on_chunk: tauri::ipc::Channel<String>) -> Result<u16, String> {
    use futures_util::StreamExt;
    let stop = std::sync::Arc::new(tokio::sync::Notify::new());
    streams.0.lock().unwrap().insert(id.clone(), stop.clone());
    let run = async {
        let response = client.post(format!("http://127.0.0.1:{}/v1/chat/completions", port())).header("content-type", "application/json").body(body).send().await.map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        let mut stream = response.bytes_stream();
        // a piece can end in the middle of a character: keep the unfinished bytes for the next one
        let mut carry: Vec<u8> = Vec::new();
        loop {
            tokio::select! {
                _ = stop.notified() => break,
                piece = stream.next() => match piece {
                    Some(Ok(bytes)) => {
                        carry.extend_from_slice(&bytes);
                        let whole = match std::str::from_utf8(&carry) {
                            Ok(_) => carry.len(),
                            Err(e) => e.valid_up_to(),
                        };
                        if whole > 0 {
                            let text = String::from_utf8_lossy(&carry[..whole]).into_owned();
                            carry.drain(..whole);
                            on_chunk.send(text).map_err(|e| e.to_string())?;
                        }
                    }
                    Some(Err(e)) => return Err(e.to_string()),
                    None => break,
                },
            }
        }
        Ok(status)
    };
    let result = run.await;
    streams.0.lock().unwrap().remove(&id);
    result
}

#[tauri::command]
fn chat_cancel(streams: tauri::State<'_, Streams>, id: String) {
    if let Some(stop) = streams.0.lock().unwrap().get(&id) {
        stop.notify_waiters();
    }
}

async fn answering(client: &reqwest::Client) -> bool {
    client.get(format!("http://127.0.0.1:{}/health", port())).timeout(Duration::from_millis(800)).send().await.is_ok_and(|r| r.status().is_success())
}

/// The service program: `EIGHTGENT_BIN`, the copy inside the app, or (when developing) the one
/// built in this repository.
fn service_program(app: &tauri::AppHandle) -> Option<PathBuf> {
    let name = if cfg!(windows) { "8gent.exe" } else { "8gent" };
    let mut places: Vec<PathBuf> = vec![];
    if let Some(p) = std::env::var_os("EIGHTGENT_BIN") {
        places.push(PathBuf::from(p));
    }
    if let Ok(dir) = app.path().resource_dir() {
        places.push(dir.join("bin").join(name));
    }
    places.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/release").join(name));
    places.into_iter().find(|p| p.is_file())
}

async fn start(app: &tauri::AppHandle, client: &reqwest::Client) -> Result<String, String> {
    if answering(client).await {
        return Ok("already running".into());
    }
    // set to start at login: the login agent owns the service, so ask it to start it (unless this
    // window was pointed at another data folder, whose service the login agent does not run)
    if std::env::var_os("EIGHTGENT_HOME").is_none() && eightgent::service::installed() && eightgent::service::kickstart() {
        for _ in 0..80 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if answering(client).await {
                return Ok("started".into());
            }
        }
    }
    let program = service_program(app).ok_or("The 8gent service program was not found next to the app.")?;
    let paths = Paths::from_env();
    paths.ensure().map_err(|e| format!("{}: {e}", paths.home.display()))?;
    let log = std::fs::OpenOptions::new().create(true).append(true).open(paths.logs().join("service.log")).map_err(|e| e.to_string())?;
    let mut command = std::process::Command::new(&program);
    command.arg("serve").stdin(std::process::Stdio::null()).stdout(log.try_clone().map_err(|e| e.to_string())?).stderr(log);
    // its own process group: the service outlives this window
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    // Windows: no console window, and not tied to this one
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000 | 0x0000_0008); // CREATE_NO_WINDOW | DETACHED_PROCESS
    }
    command.spawn().map_err(|e| format!("{} could not be started: {e}", program.display()))?;
    for _ in 0..80 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if answering(client).await {
            return Ok("started".into());
        }
    }
    Err(format!("The service was started but is not answering. Its output is in {}.", paths.logs().join("service.log").display()))
}

#[tauri::command]
async fn start_service(app: tauri::AppHandle, client: tauri::State<'_, reqwest::Client>) -> Result<String, String> {
    start(&app, &client).await
}

/// Where 8gent's own releases are.
const REPO: &str = "majieddd/8gent";

#[derive(serde::Serialize)]
struct AppUpdate {
    current: String,
    latest: String,
    available: bool,
    /// This build can install it by itself (macOS); elsewhere the release page has the installer.
    can_install: bool,
    page: String,
}

fn app_file(version: &str) -> String {
    format!("8gent-{version}-{}.app.zip", eightgent::manifest::platform())
}

fn check_update(current: &str) -> Result<AppUpdate, String> {
    let (latest, page, files) = eightgent::updates::latest_release(REPO)?;
    let available = eightgent::manifest::compare_versions(&latest, current) == std::cmp::Ordering::Greater;
    let can_install = cfg!(target_os = "macos") && files.iter().any(|f| f.name == app_file(&latest) && f.sha256.is_some());
    Ok(AppUpdate { current: current.to_string(), latest, available, can_install, page })
}

/// Is there a newer 8gent than this one?
#[tauri::command]
async fn app_update_check(app: tauri::AppHandle) -> Result<AppUpdate, String> {
    let current = app.package_info().version.to_string();
    tauri::async_runtime::spawn_blocking(move || check_update(&current)).await.map_err(|e| e.to_string())?
}

/// The app bundle this program runs from.
#[cfg(target_os = "macos")]
fn this_bundle() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    exe.ancestors().find(|p| p.extension().is_some_and(|x| x == "app")).map(|p| p.to_path_buf()).ok_or_else(|| "This copy of 8gent is not inside an app bundle, so it cannot replace itself.".to_string())
}

/// Download the newest 8gent and make sure of it: the SHA-256 GitHub publishes, that it is 8gent,
/// and that it is the version the release names. Returns that version and where the new app waits.
#[cfg(target_os = "macos")]
fn stage_update(current: &str, paths: &Paths) -> Result<(String, PathBuf), String> {
    use eightgent::updates;
    let stage = paths.home.join("updates").join("staged");
    let (latest, _, files) = updates::latest_release(REPO)?;
    if eightgent::manifest::compare_versions(&latest, current) != std::cmp::Ordering::Greater {
        return Err(format!("8gent {current} is the newest version."));
    }
    let file = files.iter().find(|f| f.name == app_file(&latest)).ok_or_else(|| format!("Release {latest} has no build for this kind of Mac."))?;
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    let zip = stage.join(&file.name);
    updates::download_verified(file, &zip)?;
    updates::run(std::path::Path::new("/usr/bin/ditto"), &["-x", "-k", &zip.to_string_lossy(), &stage.to_string_lossy()], Duration::from_secs(300))?;
    let fresh = stage.join("8gent.app");
    let plist = fresh.join("Contents/Info.plist");
    let read = |key: &str| updates::run(std::path::Path::new("/usr/bin/plutil"), &["-extract", key, "raw", &plist.to_string_lossy()], Duration::from_secs(10)).map(|s| s.trim().to_string());
    if read("CFBundleIdentifier")? != "app.kr8.eightgent" {
        return Err("The download is not 8gent. Nothing was installed.".into());
    }
    let got = read("CFBundleShortVersionString")?;
    if got != latest {
        return Err(format!("The download says it is {got}; the release says {latest}. Nothing was installed."));
    }
    Ok((latest, fresh))
}

/// Start the helper that, once this program has ended, keeps a copy of the app that is here, puts
/// the staged one in its place (the old one back if that fails), restarts the service on the new
/// program and opens the app again.
#[cfg(target_os = "macos")]
fn swap_when_this_ends(bundle: &std::path::Path, staged: &std::path::Path, backup: &std::path::Path, reopen: bool) -> Result<(), String> {
    let q = |p: &std::path::Path| format!("'{}'", p.to_string_lossy().replace('\'', "'\\''"));
    // the login item runs the default data folder's service; a window pointed elsewhere leaves it alone
    let own_service = std::env::var_os("EIGHTGENT_HOME").is_none();
    let script = format!(
        r#"while kill -0 {pid} 2>/dev/null; do sleep 0.3; done
/usr/bin/curl -s -m 5 -X POST -H 'Content-Type: application/json' -d '{{}}' http://127.0.0.1:{port}/8gent/v1/shutdown >/dev/null 2>&1
sleep 2
rm -rf {backup}; mkdir -p {backup}
/usr/bin/ditto {bundle} {backup}/8gent.app || exit 1
rm -rf {bundle} && /usr/bin/ditto {staged} {bundle} || {{ /usr/bin/ditto {backup}/8gent.app {bundle}; }}
/usr/bin/xattr -dr com.apple.quarantine {bundle} 2>/dev/null
rm -rf {stage}
{kick}
{open}
"#,
        pid = std::process::id(),
        port = port(),
        backup = q(backup),
        bundle = q(bundle),
        staged = q(staged),
        stage = q(staged.parent().unwrap_or(staged)),
        kick = if own_service { format!("[ -f \"$HOME/Library/LaunchAgents/{label}.plist\" ] && /bin/launchctl kickstart \"gui/$(id -u)/{label}\" >/dev/null 2>&1", label = eightgent::service::LABEL) } else { "true".to_string() },
        open = if reopen { format!("/usr/bin/open {}", q(bundle)) } else { "true".to_string() },
    );
    let mut helper = std::process::Command::new("/bin/bash");
    helper.args(["-c", &script]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    {
        use std::os::unix::process::CommandExt;
        helper.process_group(0);
    }
    helper.spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Download the newest 8gent, check it, and put it in place of this one: this app quits, a helper
/// keeps a copy of the version that was here, swaps the new one in, restarts the service on it and
/// opens the app again.
#[tauri::command]
async fn app_update_install(app: tauri::AppHandle) -> Result<String, String> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Err("On this system, install the new version from the release page.".to_string())
    }
    #[cfg(target_os = "macos")]
    {
        let current = app.package_info().version.to_string();
        let bundle = this_bundle()?;
        let paths = Paths::from_env();
        let backup = paths.home.join("updates").join(format!("app-before-{current}"));
        let (version, staged) = tauri::async_runtime::spawn_blocking(move || stage_update(&current, &paths)).await.map_err(|e| e.to_string())??;
        swap_when_this_ends(&bundle, &staged, &backup, true)?;
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_millis(600)).await;
            handle.exit(0);
        });
        Ok(version)
    }
}

/// The updater without the window, for checking it: `--update-check` says what a check finds,
/// `--update-install` downloads, checks and swaps (without opening the app afterwards).
/// `--as-version X` runs either as if this were version X.
fn headless(args: &[String]) -> Option<i32> {
    let has = |flag: &str| args.iter().any(|a| a == flag);
    if !has("--update-check") && !has("--update-install") {
        return None;
    }
    let current = args.iter().position(|a| a == "--as-version").and_then(|i| args.get(i + 1)).cloned().unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    if has("--update-check") {
        return Some(match check_update(&current) {
            Ok(u) => {
                println!("this is {}, the newest release is {}: {}{}", u.current, u.latest, if u.available { "update available" } else { "up to date" }, if u.available && !u.can_install { " (install it from the release page)" } else { "" });
                0
            }
            Err(e) => {
                eprintln!("8gent: {e}");
                1
            }
        });
    }
    #[cfg(target_os = "macos")]
    {
        let paths = Paths::from_env();
        let run = || -> Result<String, String> {
            let bundle = this_bundle()?;
            let (version, staged) = stage_update(&current, &paths)?;
            swap_when_this_ends(&bundle, &staged, &paths.home.join("updates").join(format!("app-before-{current}")), false)?;
            Ok(version)
        };
        Some(match run() {
            Ok(v) => {
                println!("{v} is downloaded and checked; it is put in place as this program ends");
                0
            }
            Err(e) => {
                eprintln!("8gent: {e}");
                1
            }
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("8gent: on this system, install the new version from the release page");
        Some(1)
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = headless(&args) {
        std::process::exit(code);
    }
    let client = reqwest::Client::builder().connect_timeout(Duration::from_secs(3)).build().expect("http client");
    tauri::Builder::default()
        .manage(client.clone())
        .manage(Streams::default())
        .setup(move |app| {
            // opening the window is enough: start the service if it is not there yet
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = start(&handle, &client).await {
                    eprintln!("8gent: {e}");
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![api, start_service, chat_stream, chat_cancel, app_update_check, app_update_install])
        .run(tauri::generate_context!())
        .expect("the 8gent window could not start");
}
