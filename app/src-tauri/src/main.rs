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
    // set to start at login: the login agent owns the service, so ask it to start it
    if eightgent::service::installed() && eightgent::service::kickstart() {
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

fn main() {
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
        .invoke_handler(tauri::generate_handler![api, start_service, chat_stream, chat_cancel])
        .run(tauri::generate_context!())
        .expect("the 8gent window could not start");
}
