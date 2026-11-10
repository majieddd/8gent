//! Carrying a request to the engine and its answer back: wake the engine if it is not loaded, shape
//! the request for this loader, and turn the engine's own way of saying "the prompt does not fit" into
//! the standard error that agents know how to recover from.

use crate::engine::Running;
use crate::normalize::{self, error_body};
use crate::server::{ApiErr, App};
use crate::stats::{RequestRecord, now};
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::OwnedRwLockReadGuard;

/// How long a stream's first lines are held back, waiting to learn whether the engine is about to
/// report an error. Past this the client gets its headers (a slow prompt read must not look dead).
const HOLD: Duration = Duration::from_secs(2);

const OPENAI_CHAT: &[&str] = &["/v1/chat/completions", "/v1/completions"];

pub async fn handle(app: Arc<App>, method: Method, path_and_query: String, headers: HeaderMap, body: Bytes) -> Response {
    let path = path_and_query.split('?').next().unwrap_or("").to_string();
    app.touch();

    // a read (GET /v1/something) is answered by a loaded engine but never loads one
    if method != Method::POST && method != Method::PUT && method != Method::PATCH {
        let Some(run) = app.engine.current() else {
            return ApiErr::unavailable("No model is loaded. A request to /v1/chat/completions loads the selected one.").into_response();
        };
        let _guard = app.gate.clone().read_owned().await;
        return match app.http.request(method, format!("{}{}", run.base_url, path_and_query)).send().await {
            Ok(resp) => passthrough(resp).await,
            Err(e) => ApiErr::new(StatusCode::BAD_GATEWAY, format!("The engine did not answer: {e}"), "upstream_error", None).into_response(),
        };
    }

    let mut doc: Value = match serde_json::from_slice(&body) {
        Ok(v @ Value::Object(_)) => v,
        _ => return ApiErr::bad_request("The request body is not a JSON object.").into_response(),
    };
    let target = match app.resolve(doc.get("model").and_then(|m| m.as_str())) {
        Ok(t) => t,
        Err(e) => return e.into_response(),
    };
    let Some(manifest) = app.manifest(&target.selection.loader) else {
        return ApiErr::unavailable(format!("There is no loader called \"{}\".", target.selection.loader)).into_response();
    };
    let api = manifest.api.clone();
    let chat = OPENAI_CHAT.contains(&path.as_str());
    let stream = doc.get("stream").and_then(|s| s.as_bool()).unwrap_or(false);
    let mut strip_usage = false;
    if chat {
        if api.role_normalize {
            normalize::normalize_roles(&mut doc);
        }
        normalize::apply_thinking(&mut doc, &api, target.selection.thinking);
        // 8gent wants the token counts of every stream; a client that did not ask does not see them
        if stream && api.stream_usage && doc.get("stream_options").and_then(|o| o.get("include_usage")).and_then(|u| u.as_bool()) != Some(true) {
            let opts = doc.as_object_mut().unwrap().entry("stream_options").or_insert_with(|| serde_json::json!({}));
            if let Some(o) = opts.as_object_mut() {
                o.insert("include_usage".into(), Value::Bool(true));
                strip_usage = true;
            }
        }
    }
    let estimate = normalize::estimate_tokens(&doc);

    app.stats.begin();
    let began = Instant::now();
    let mut rec = RequestRecord { at: now(), loader: target.selection.loader.clone(), model: target.selection.model.clone(), path: path.clone(), stream, ..Default::default() };

    let acquired = if target.small { app.acquire_small().await } else { app.acquire(&target.selection).await };
    let (run, guard) = match acquired {
        Ok(x) => x,
        Err(e) => return finish_error(&app, rec, began, e),
    };
    app.touch();
    rec.loader_version = run.loader_version.clone();
    doc["model"] = Value::String(run.engine_model.clone());
    let payload = match serde_json::to_vec(&doc) {
        Ok(p) => p,
        Err(e) => return finish_error(&app, rec, began, ApiErr::bad_request(e.to_string())),
    };

    let mut request = app.http.request(method, format!("{}{}", run.base_url, path_and_query)).header(header::CONTENT_TYPE, "application/json").body(payload);
    for name in ["anthropic-version", "anthropic-beta", "accept"] {
        if let Some(v) = headers.get(name) {
            request = request.header(name, v);
        }
    }
    let resp = match request.send().await {
        Ok(r) => r,
        Err(e) => {
            // a managed engine that died is noticed here; say so rather than a bare connection error
            let died = app.engine.current().is_none();
            let msg = if died { app.engine.last_error().unwrap_or_else(|| "The engine stopped.".into()) } else { format!("The engine did not answer: {e}") };
            return finish_error(&app, rec, began, ApiErr::new(StatusCode::BAD_GATEWAY, msg, "upstream_error", Some("engine_unavailable")));
        }
    };

    let status = resp.status();
    let is_sse = resp.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|v| v.starts_with("text/event-stream"));
    let ctx = Shared { app: app.clone(), run, requested: target.requested, estimate, markers: api.overflow_markers.clone() };
    if is_sse && status.is_success() {
        stream_response(ctx, resp, rec, began, guard, strip_usage, chat).await
    } else {
        let out = json_response(&ctx, resp, &mut rec, chat).await;
        rec.duration_ms = began.elapsed().as_millis() as u64;
        app.stats.finish(rec);
        drop(guard);
        out
    }
}

struct Shared {
    app: Arc<App>,
    run: Running,
    requested: String,
    estimate: u64,
    markers: Vec<String>,
}

impl Shared {
    /// The standard overflow answer, counted.
    fn overflow(&self, engine_error: &Value, rec: &mut RequestRecord) -> Value {
        let exact = normalize::overflow_tokens(engine_error, self.run.context);
        let tokens = exact.or(Some(self.estimate).filter(|n| *n > 0));
        self.app.stats.overflow(self.run.context, tokens);
        // the request log shows how long the prompt that did not fit was
        rec.finish = "overflow".into();
        rec.prompt_tokens = tokens;
        normalize::overflow_error(self.run.context, tokens, exact.is_some())
    }
}

fn finish_error(app: &App, mut rec: RequestRecord, began: Instant, e: ApiErr) -> Response {
    rec.finish = "error".into();
    rec.status = e.status.as_u16();
    rec.duration_ms = began.elapsed().as_millis() as u64;
    app.stats.finish(rec);
    e.into_response()
}

async fn passthrough(resp: reqwest::Response) -> Response {
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let ctype = resp.headers().get(header::CONTENT_TYPE).cloned();
    let bytes = resp.bytes().await.unwrap_or_default();
    let mut out = Response::new(Body::from(bytes));
    *out.status_mut() = status;
    if let Some(c) = ctype {
        out.headers_mut().insert(header::CONTENT_TYPE, c);
    }
    out
}

fn json_out(status: StatusCode, body: &Value) -> Response {
    let mut out = Response::new(Body::from(serde_json::to_vec(body).unwrap_or_default()));
    *out.status_mut() = status;
    out.headers_mut().insert(header::CONTENT_TYPE, header::HeaderValue::from_static("application/json"));
    out
}

/// A whole (not streamed) answer: errors made standard, the model name written back, usage recorded.
async fn json_response(ctx: &Shared, resp: reqwest::Response, rec: &mut RequestRecord, chat: bool) -> Response {
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let ctype = resp.headers().get(header::CONTENT_TYPE).cloned();
    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            rec.finish = "error".into();
            rec.status = 502;
            return json_out(StatusCode::BAD_GATEWAY, &error_body(format!("The engine's answer broke off: {e}"), "upstream_error", None));
        }
    };
    let Ok(mut doc) = serde_json::from_slice::<Value>(&bytes) else {
        rec.finish = if status.is_success() { "ok" } else { "error" }.into();
        rec.status = status.as_u16();
        let mut out = Response::new(Body::from(bytes));
        *out.status_mut() = status;
        if let Some(c) = ctype {
            out.headers_mut().insert(header::CONTENT_TYPE, c);
        }
        return out;
    };
    if normalize::error_message(&doc).is_some() {
        if normalize::is_overflow(&doc, &ctx.markers) {
            rec.status = 400;
            return json_out(StatusCode::BAD_REQUEST, &ctx.overflow(&doc, rec));
        }
        // some engines (uzu) report an error in a 200 answer; a client must see it as an error
        let status = if status.is_success() { StatusCode::INTERNAL_SERVER_ERROR } else { status };
        rec.finish = "error".into();
        rec.status = status.as_u16();
        return json_out(status, &doc);
    }
    if chat && doc.get("model").is_some() {
        doc["model"] = Value::String(ctx.requested.clone());
    }
    if let Some(usage) = doc.get("usage") {
        rec.prompt_tokens = usage.get("prompt_tokens").or(usage.get("input_tokens")).and_then(|v| v.as_u64());
        rec.completion_tokens = usage.get("completion_tokens").or(usage.get("output_tokens")).and_then(|v| v.as_u64());
    }
    // a whole answer arrives at once: the monitor counts its tokens in this second
    ctx.app.live_tokens.fetch_add(rec.completion_tokens.unwrap_or(0), std::sync::atomic::Ordering::Relaxed);
    // llama.cpp reports its own decode speed
    rec.tokens_per_second = doc.get("timings").and_then(|t| t.get("predicted_per_second")).and_then(|v| v.as_f64());
    rec.finish = if status.is_success() { "ok" } else { "error" }.into();
    rec.status = status.as_u16();
    json_out(status, &doc)
}

enum Hold {
    Wait,
    Commit,
    Error(Value),
}

/// Look at the stream's first complete lines: an error event means the request failed before any
/// answer; the first piece of an answer (or a second event of any kind) means it is under way.
fn inspect(held: &[u8]) -> Hold {
    let Some(last_newline) = held.iter().rposition(|b| *b == b'\n') else { return Hold::Wait };
    let mut events = 0;
    for line in String::from_utf8_lossy(&held[..last_newline]).lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with("event:") && !line.contains("message_start") && !line.contains("ping") {
            if line.contains("error") {
                continue; // its data line follows
            }
            return Hold::Commit;
        }
        let Some(payload) = line.strip_prefix("data:").map(str::trim_start) else { continue };
        if payload == "[DONE]" {
            return Hold::Commit;
        }
        let Ok(ev) = serde_json::from_str::<Value>(payload) else { return Hold::Commit };
        if ev.get("error").is_some_and(|e| !e.is_null()) {
            return Hold::Error(ev);
        }
        events += 1;
        let delta = ev.get("choices").and_then(|c| c.get(0)).and_then(|c| c.get("delta"));
        let has = |k: &str| delta.and_then(|d| d.get(k)).is_some_and(|v| match v {
            Value::String(s) => !s.is_empty(),
            Value::Array(a) => !a.is_empty(),
            Value::Null => false,
            _ => true,
        });
        if has("content") || has("reasoning_content") || has("reasoning") || has("tool_calls") || events >= 2 {
            return Hold::Commit;
        }
    }
    Hold::Wait
}

async fn stream_response(ctx: Shared, resp: reqwest::Response, mut rec: RequestRecord, began: Instant, guard: OwnedRwLockReadGuard<()>, strip_usage: bool, chat: bool) -> Response {
    let mut upstream: BoxStream<'static, reqwest::Result<Bytes>> = resp.bytes_stream().boxed();
    let mut held: Vec<u8> = Vec::new();
    let mut ended = false;
    let deadline = tokio::time::Instant::now() + HOLD;
    if chat {
        loop {
            match inspect(&held) {
                Hold::Commit => break,
                Hold::Error(ev) => {
                    let (status, body) = if normalize::is_overflow(&ev, &ctx.markers) {
                        (StatusCode::BAD_REQUEST, ctx.overflow(&ev, &mut rec))
                    } else {
                        rec.finish = "error".into();
                        (StatusCode::INTERNAL_SERVER_ERROR, ev)
                    };
                    rec.status = status.as_u16();
                    rec.duration_ms = began.elapsed().as_millis() as u64;
                    ctx.app.stats.finish(rec);
                    return json_out(status, &body);
                }
                Hold::Wait => {}
            }
            match tokio::time::timeout_at(deadline, upstream.next()).await {
                Ok(Some(Ok(chunk))) => held.extend_from_slice(&chunk),
                Ok(Some(Err(e))) => {
                    return finish_error(&ctx.app, rec, began, ApiErr::new(StatusCode::BAD_GATEWAY, format!("The engine's answer broke off: {e}"), "upstream_error", None));
                }
                Ok(None) => {
                    ended = true;
                    break;
                }
                Err(_) => break, // held long enough
            }
        }
    }

    rec.status = 200;
    let mut shaper = Shaper { ctx, rec: Some(rec), began, first: None, pieces: 0, buf: Vec::new(), strip_usage, drop_blank: false, done: false, rewrite: chat, _guard: guard };
    let first = shaper.feed(&held);
    let body = futures_util::stream::unfold((upstream, shaper, Some(first), ended), |(mut up, mut shaper, first, ended)| async move {
        if let Some(f) = first.filter(|f| !f.is_empty()) {
            return Some((Ok::<Bytes, std::io::Error>(Bytes::from(f)), (up, shaper, None, ended)));
        }
        if ended {
            return None;
        }
        loop {
            match up.next().await {
                Some(Ok(chunk)) => {
                    let out = shaper.feed(&chunk);
                    if !out.is_empty() {
                        return Some((Ok(Bytes::from(out)), (up, shaper, None, false)));
                    }
                }
                Some(Err(_)) => {
                    shaper.broke();
                    return None;
                }
                None => {
                    let out = shaper.end();
                    return (!out.is_empty()).then(|| (Ok(Bytes::from(out)), (up, shaper, None, true)));
                }
            }
        }
    });
    let mut out = Response::new(Body::from_stream(body));
    let h = out.headers_mut();
    h.insert(header::CONTENT_TYPE, header::HeaderValue::from_static("text/event-stream"));
    h.insert(header::CACHE_CONTROL, header::HeaderValue::from_static("no-cache"));
    out
}

/// Passes a stream's lines on, one pass and no JSON parsing except where a line carries the token
/// counts: writes the client's model name back, notes when the answer began, drops the usage event
/// the client did not ask for. Dropped before the end, it records the request as cancelled.
struct Shaper {
    ctx: Shared,
    rec: Option<RequestRecord>,
    began: Instant,
    first: Option<Instant>,
    pieces: u64,
    buf: Vec<u8>,
    strip_usage: bool,
    drop_blank: bool,
    done: bool,
    rewrite: bool,
    _guard: OwnedRwLockReadGuard<()>,
}

impl Shaper {
    fn feed(&mut self, chunk: &[u8]) -> Vec<u8> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::with_capacity(chunk.len() + 16);
        while let Some(nl) = self.buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=nl).collect();
            self.line(&line[..line.len() - 1], &mut out);
        }
        out
    }

    fn end(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        if !self.buf.is_empty() {
            let rest = std::mem::take(&mut self.buf);
            self.line(&rest, &mut out);
        }
        self.done = true;
        out
    }

    fn broke(&mut self) {
        if let Some(rec) = self.rec.as_mut() {
            rec.finish = "error".into();
        }
    }

    fn line(&mut self, raw: &[u8], out: &mut Vec<u8>) {
        let text = String::from_utf8_lossy(raw);
        let trimmed = text.trim_end_matches('\r');
        if self.drop_blank && trimmed.is_empty() {
            self.drop_blank = false;
            return;
        }
        self.drop_blank = false;
        if let Some(payload) = trimmed.strip_prefix("data:").map(str::trim_start) {
            if payload == "[DONE]" {
                self.done = true;
            } else {
                if payload.contains("\"usage\"") {
                    if let Ok(ev) = serde_json::from_str::<Value>(payload) {
                        if let (Some(usage), Some(rec)) = (ev.get("usage").filter(|u| u.is_object()), self.rec.as_mut()) {
                            rec.prompt_tokens = usage.get("prompt_tokens").or(usage.get("input_tokens")).and_then(|v| v.as_u64()).or(rec.prompt_tokens);
                            rec.completion_tokens = usage.get("completion_tokens").or(usage.get("output_tokens")).and_then(|v| v.as_u64()).or(rec.completion_tokens);
                            if self.strip_usage && ev.get("choices").and_then(|c| c.as_array()).is_some_and(|c| c.is_empty()) {
                                self.drop_blank = true;
                                return;
                            }
                        }
                    }
                }
                if payload.contains("\"delta\"") && (payload.contains("\"content\":\"") || payload.contains("\"content\": \"") || payload.contains("reasoning") || payload.contains("\"tool_calls\"")) {
                    self.pieces += 1;
                    self.ctx.app.live_tokens.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    self.first.get_or_insert_with(Instant::now);
                }
                if self.rewrite {
                    if let Some(rewritten) = normalize::rewrite_model_field(&text, &self.ctx.requested) {
                        out.extend_from_slice(rewritten.as_bytes());
                        out.push(b'\n');
                        return;
                    }
                }
            }
        }
        out.extend_from_slice(raw);
        out.push(b'\n');
    }
}

impl Drop for Shaper {
    fn drop(&mut self) {
        let Some(mut rec) = self.rec.take() else { return };
        rec.duration_ms = self.began.elapsed().as_millis() as u64;
        if rec.finish.is_empty() {
            rec.finish = if self.done { "ok" } else { "cancelled" }.into();
        }
        if let Some(first) = self.first {
            rec.first_token_ms = Some(first.duration_since(self.began).as_millis() as u64);
            let writing = first.elapsed().as_secs_f64();
            let tokens = rec.completion_tokens.unwrap_or(self.pieces);
            if writing > 0.05 && tokens > 1 {
                rec.tokens_per_second = Some(((tokens - 1) as f64 / writing * 10.0).round() / 10.0);
            }
        }
        self.ctx.app.stats.finish(rec);
        self.ctx.app.touch();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hold_decisions() {
        assert!(matches!(inspect(b"data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n"), Hold::Wait));
        assert!(matches!(inspect(b"data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n"), Hold::Commit));
        assert!(matches!(inspect(b"data:{\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\ndata:{\"error\":{\"message\":\"Context overflow\"}}\n"), Hold::Error(_)));
        assert!(matches!(inspect(b"data: {\"choices\":[{\"delta\":{\"content\":\"\"}}]}\n\n: keep-alive\n"), Hold::Wait));
        assert!(matches!(inspect(b"data: {\"choices\":[{\"de"), Hold::Wait));
        assert!(matches!(inspect(b"data: [DONE]\n"), Hold::Commit));
    }
}
