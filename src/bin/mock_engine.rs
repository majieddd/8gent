//! A stand-in engine for 8gent's conformance tests: an OpenAI-style server whose quirks are switched
//! on by flags, so each real loader's behaviour can be reproduced without a model or a GPU.
//!
//!   --port N            listen on 127.0.0.1:N
//!   --name NAME         answers say "reply from NAME"
//!   --model-id ID       the model name it reports and expects
//!   --strict-roles      refuse a history that resumes with the assistant (uzu's template), in a 200 answer
//!   --overflow STYLE    "openai": a 400 with the standard code; "uzu": an error inside a 200 answer / stream
//!   --compact-sse       write "data:{" without the space (uzu)
//!   --load-delay MS     wait before listening, like a model loading
//!   --fail              print a reason and exit 3, like a model that will not load
//!
//! A prompt containing OVERFLOW triggers the overflow answer; PREFILL3 a 3 s pause after the first
//! stream event. GET /mock/last returns the last request body it received.

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct Mock {
    name: String,
    model_id: String,
    strict_roles: bool,
    overflow: String,
    compact: bool,
    last: Mutex<Value>,
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().any(|a| a == name);
    if flag("--fail") {
        println!("mock-engine: cannot load the model (not enough memory)");
        std::process::exit(3);
    }
    let port: u16 = arg(&args, "--port").and_then(|p| p.parse().ok()).expect("--port");
    if let Some(ms) = arg(&args, "--load-delay").and_then(|p| p.parse().ok()) {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
    let mock = Arc::new(Mock {
        name: arg(&args, "--name").unwrap_or("mock".into()),
        model_id: arg(&args, "--model-id").unwrap_or("mock-model".into()),
        strict_roles: flag("--strict-roles"),
        overflow: arg(&args, "--overflow").unwrap_or("openai".into()),
        compact: flag("--compact-sse"),
        last: Mutex::new(Value::Null),
    });
    println!("mock-engine: {} serving {} on 127.0.0.1:{port}", mock.name, mock.model_id);
    let app = Router::new()
        .route("/health", get(|| async { Json(json!({"status": "ok"})) }))
        .route("/v1/models", get(models))
        .route("/mock/last", get(last))
        .route("/v1/chat/completions", post(chat))
        .with_state(mock);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.expect("bind");
    axum::serve(listener, app).await.expect("serve");
}

async fn models(State(m): State<Arc<Mock>>) -> Json<Value> {
    Json(json!({"object": "list", "data": [{"id": m.model_id, "object": "model"}]}))
}

async fn last(State(m): State<Arc<Mock>>) -> Json<Value> {
    Json(m.last.lock().unwrap().clone())
}

fn json_response(status: StatusCode, body: Value) -> Response {
    (status, Json(body)).into_response()
}

async fn chat(State(m): State<Arc<Mock>>, body: Bytes) -> Response {
    let doc: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    *m.last.lock().unwrap() = doc.clone();
    let stream = doc["stream"].as_bool().unwrap_or(false);
    let text = doc["messages"].to_string();

    if doc["model"].as_str() != Some(m.model_id.as_str()) {
        return json_response(StatusCode::NOT_FOUND, json!({"error": {"message": format!("model '{}' not found", doc["model"].as_str().unwrap_or("")), "code": "model_not_found"}}));
    }
    if m.strict_roles {
        let roles: Vec<&str> = doc["messages"].as_array().into_iter().flatten().filter_map(|x| x["role"].as_str()).collect();
        let first = roles.iter().find(|r| !matches!(**r, "system" | "developer")).copied().unwrap_or("user");
        if first != "user" {
            return json_response(StatusCode::OK, json!({"error": {"message": format!("Expected one of [developer, user] after 'system', got '{first}'")}}));
        }
    }
    let overflow = text.contains("OVERFLOW");
    if overflow && m.overflow == "openai" {
        return json_response(StatusCode::BAD_REQUEST, json!({"error": {"message": "This server's maximum context length is 65536 tokens, but the rendered prompt has 70123 tokens and leaves no room for a reply.",
            "type": "invalid_request_error", "param": "messages", "code": "context_length_exceeded"}}));
    }
    if overflow && !stream {
        return json_response(StatusCode::OK, json!({"error": {"message": "Context overflow: the prompt is longer than the context"}}));
    }

    let reply = format!("reply from {}", m.name);
    let usage = json!({"prompt_tokens": 12, "completion_tokens": 3, "total_tokens": 15});
    if !stream {
        return json_response(StatusCode::OK, json!({"id": "chatcmpl-mock", "object": "chat.completion", "model": m.model_id,
            "choices": [{"index": 0, "message": {"role": "assistant", "content": reply}, "finish_reason": "stop"}], "usage": usage}));
    }

    let sep = if m.compact { "data:" } else { "data: " };
    let event = |v: Value| format!("{sep}{v}\n\n");
    let chunk = |delta: Value, finish: Value| json!({"id": "chatcmpl-mock", "object": "chat.completion.chunk", "model": m.model_id, "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]});
    let mut events: Vec<(String, u64)> = vec![(event(chunk(json!({"role": "assistant"}), Value::Null)), 0)];
    if overflow {
        events.push((event(json!({"error": {"message": "Context overflow: the prompt is longer than the context"}})), 0));
    } else {
        let pause = if text.contains("PREFILL3") { 3000 } else { 0 };
        let words: Vec<&str> = reply.split(' ').collect();
        for (i, word) in words.iter().enumerate() {
            let piece = if i == 0 { word.to_string() } else { format!(" {word}") };
            events.push((event(chunk(json!({"content": piece}), Value::Null)), if i == 0 { pause } else { 20 }));
        }
        events.push((event(chunk(json!({}), json!("stop"))), 0));
        if doc["stream_options"]["include_usage"].as_bool() == Some(true) {
            events.push((event(json!({"id": "chatcmpl-mock", "object": "chat.completion.chunk", "model": m.model_id, "choices": [], "usage": usage})), 0));
        }
        events.push((format!("{sep}[DONE]\n\n"), 0));
    }
    let body = futures_util::stream::unfold(events.into_iter(), |mut it| async move {
        let (text, wait_ms) = it.next()?;
        if wait_ms > 0 {
            tokio::time::sleep(Duration::from_millis(wait_ms)).await;
        }
        Some((Ok::<Bytes, std::io::Error>(Bytes::from(text)), it))
    });
    let mut out = Response::new(Body::from_stream(body));
    out.headers_mut().insert(header::CONTENT_TYPE, header::HeaderValue::from_static("text/event-stream"));
    out
}
