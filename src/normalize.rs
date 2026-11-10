//! Shaping a request for the engine that will answer it, and its errors for the client that asked.
//! Every loader's API differs a little; clients should see one behaviour.

use crate::manifest::Api;
use serde_json::{Value, json};

/// Some chat templates (uzu's) want a user turn right after the system messages. A client that has
/// compressed its history can send one that resumes with the assistant; put a short user turn first.
pub fn normalize_roles(doc: &mut Value) -> bool {
    let Some(msgs) = doc.get_mut("messages").and_then(|m| m.as_array_mut()) else { return false };
    let role = |m: &Value| m.get("role").and_then(|r| r.as_str()).unwrap_or("").to_string();
    let mut i = 0;
    while i < msgs.len() && matches!(role(&msgs[i]).as_str(), "system" | "developer") {
        i += 1;
    }
    if i < msgs.len() && matches!(role(&msgs[i]).as_str(), "assistant" | "tool") {
        msgs.insert(i, json!({"role": "user", "content": "Continue."}));
        return true;
    }
    false
}

/// A rough prompt size for a request an engine refused without a count: about 3.7 characters a token.
pub fn estimate_tokens(doc: &Value) -> u64 {
    let len = |k: &str| doc.get(k).map(|v| v.to_string().len()).unwrap_or(0);
    ((len("messages") + len("tools") + len("prompt") + len("system")) as f64 / 3.7) as u64
}

fn explicit_thinking(doc: &Value) -> bool {
    doc.get("enable_thinking").is_some() || doc.get("chat_template_kwargs").and_then(|k| k.get("enable_thinking")).is_some()
}

/// Map `reasoning_effort` to a name the engine knows, and when the request does not choose, apply the
/// selected thinking mode in the way this engine takes it.
pub fn apply_thinking(doc: &mut Value, api: &Api, thinking: bool) {
    let Some(obj) = doc.as_object_mut() else { return };
    if !api.efforts.is_empty() {
        match obj.get("reasoning_effort").cloned() {
            Some(Value::String(e)) => {
                let e = e.trim().to_lowercase();
                let mapped = match e.as_str() {
                    "none" | "off" | "disabled" | "minimal" => api.thinking_off.clone(),
                    "max" | "maximum" => "xhigh".into(),
                    _ => e,
                };
                let known = |name: &str| api.efforts.iter().any(|x| x == name);
                let value = if known(&mapped) {
                    mapped
                } else if mapped == "xhigh" && known("high") {
                    "high".into()
                } else if known("default") {
                    "default".into()
                } else {
                    api.efforts[0].clone()
                };
                obj.insert("reasoning_effort".into(), Value::String(value));
            }
            Some(_) => {
                obj.remove("reasoning_effort");
            }
            None => {}
        }
    }
    if obj.contains_key("reasoning_effort") {
        if api.thinking == "enable_thinking" {
            // an effort and an explicit switch that disagree are refused by uzu; the effort is the finer choice
            obj.remove("enable_thinking");
            if let Some(kwargs) = obj.get_mut("chat_template_kwargs").and_then(|k| k.as_object_mut()) {
                kwargs.remove("enable_thinking");
                if kwargs.is_empty() {
                    obj.remove("chat_template_kwargs");
                }
            }
        }
        return;
    }
    if explicit_thinking(&Value::Object(obj.clone())) {
        return;
    }
    match api.thinking.as_str() {
        "enable_thinking" => {
            obj.insert("enable_thinking".into(), Value::Bool(thinking));
        }
        "chat_template_kwargs" => {
            let kwargs = obj.entry("chat_template_kwargs").or_insert_with(|| json!({}));
            if let Some(k) = kwargs.as_object_mut() {
                k.insert("enable_thinking".into(), Value::Bool(thinking));
            }
        }
        "reasoning_effort" if !thinking => {
            obj.insert("reasoning_effort".into(), Value::String(api.thinking_off.clone()));
        }
        _ => {}
    }
}

/// The message of an OpenAI-style (`{"error": {"message"}}` or `{"error": "text"}`) or
/// Anthropic-style (`{"type": "error", "error": {...}}`) error body.
pub fn error_message(body: &Value) -> Option<String> {
    let err = body.get("error")?;
    match err {
        Value::String(s) => Some(s.clone()),
        Value::Object(o) => Some(o.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string()),
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

pub fn is_overflow(body: &Value, markers: &[String]) -> bool {
    let err = body.get("error");
    if err.and_then(|e| e.get("code")).and_then(|c| c.as_str()) == Some("context_length_exceeded") {
        return true;
    }
    // llama.cpp: {"type": "exceed_context_size_error", "n_prompt_tokens": ..., "n_ctx": ...}
    if err.and_then(|e| e.get("type")).and_then(|t| t.as_str()).is_some_and(|t| t.contains("context_size") || t.contains("context_length")) {
        return true;
    }
    let Some(msg) = error_message(body) else { return false };
    let msg = msg.to_lowercase();
    markers.iter().any(|m| msg.contains(m.as_str()))
}

/// The prompt's token count when the engine states one: a field of the error, or a number followed by
/// "tokens" in its message that is not the window itself ("request (45015 tokens) exceeds the
/// available context size (16384 tokens)", "the rendered prompt has 70123 tokens").
pub fn overflow_tokens(body: &Value, context: u32) -> Option<u64> {
    let err = body.get("error")?;
    if let Some(n) = err.get("n_prompt_tokens").and_then(|v| v.as_u64()) {
        return Some(n);
    }
    let message = error_message(body)?.to_lowercase();
    let mut best: Option<u64> = None;
    let bytes = message.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b',') {
                i += 1;
            }
            let digits: String = message[start..i].chars().filter(|c| *c != ',').collect();
            if message[i..].trim_start().starts_with("token") {
                if let Ok(n) = digits.parse::<u64>() {
                    if n != context as u64 {
                        best = best.max(Some(n));
                    }
                }
            }
        } else {
            i += 1;
        }
    }
    best
}

/// The OpenAI-standard overflow error, which agents (Hermes, kr8) answer by compressing the
/// conversation and trying again.
pub fn overflow_error(context: u32, tokens: Option<u64>, exact: bool) -> Value {
    let about = match tokens {
        Some(n) if exact => format!(" However, your messages resulted in {n} tokens."),
        Some(n) => format!(" However, your messages resulted in about {n} tokens."),
        None => String::new(),
    };
    json!({"error": {
        "message": format!("This model's maximum context length is {context} tokens.{about} Please reduce the length of the messages, or raise the context window in 8gent."),
        "type": "invalid_request_error", "param": "messages", "code": "context_length_exceeded"}})
}

pub fn error_body(message: impl Into<String>, kind: &str, code: Option<&str>) -> Value {
    json!({"error": {"message": message.into(), "type": kind, "param": null, "code": code}})
}

/// Replace the value of the first top-level-looking `"model": "..."` in a line of JSON text without
/// parsing it: stream lines pass through here once per token.
pub fn rewrite_model_field(line: &str, to: &str) -> Option<String> {
    let key = line.find("\"model\"")?;
    let after_key = &line[key + 7..];
    let colon = after_key.find(':')?;
    if !after_key[..colon].trim().is_empty() {
        return None;
    }
    let after_colon = &after_key[colon + 1..];
    let open = after_colon.find('"')?;
    if !after_colon[..open].trim().is_empty() {
        return None;
    }
    let value = &after_colon[open + 1..];
    let mut end = None;
    let mut escaped = false;
    for (i, ch) in value.char_indices() {
        match ch {
            '\\' if !escaped => escaped = true,
            '"' if !escaped => {
                end = Some(i);
                break;
            }
            _ => escaped = false,
        }
    }
    let end = end?;
    let start = key + 7 + colon + 1 + open + 1;
    let escaped_to = serde_json::to_string(to).ok()?;
    Some(format!("{}{}{}", &line[..start], &escaped_to[1..escaped_to.len() - 1], &line[start + end..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api(thinking: &str) -> Api {
        Api { thinking: thinking.into(), ..Default::default() }
    }

    #[test]
    fn roles() {
        let mut doc = json!({"messages": [{"role": "system", "content": "s"}, {"role": "assistant", "content": "a"}, {"role": "user", "content": "u"}]});
        assert!(normalize_roles(&mut doc));
        assert_eq!(doc["messages"][1]["role"], "user");
        assert_eq!(doc["messages"].as_array().unwrap().len(), 4);
        let mut fine = json!({"messages": [{"role": "system", "content": "s"}, {"role": "user", "content": "u"}]});
        assert!(!normalize_roles(&mut fine));
    }

    #[test]
    fn thinking_default_applies_only_when_the_request_does_not_choose() {
        let mut doc = json!({"messages": []});
        apply_thinking(&mut doc, &api("enable_thinking"), false);
        assert_eq!(doc["enable_thinking"], false);

        let mut doc = json!({"messages": []});
        apply_thinking(&mut doc, &api("chat_template_kwargs"), false);
        assert_eq!(doc["chat_template_kwargs"]["enable_thinking"], false);

        let mut doc = json!({"enable_thinking": true});
        apply_thinking(&mut doc, &api("enable_thinking"), false);
        assert_eq!(doc["enable_thinking"], true);

        let mut doc = json!({"messages": []});
        apply_thinking(&mut doc, &Api { thinking: "reasoning_effort".into(), thinking_off: "none".into(), ..Default::default() }, false);
        assert_eq!(doc["reasoning_effort"], "none");
        let mut doc = json!({"messages": []});
        apply_thinking(&mut doc, &api("reasoning_effort"), true);
        assert!(doc.get("reasoning_effort").is_none());
    }

    #[test]
    fn efforts_map_to_what_the_engine_knows() {
        let uzu = Api { thinking: "enable_thinking".into(), thinking_off: "disabled".into(),
            efforts: ["disabled", "default", "low", "medium", "high", "xhigh"].map(String::from).to_vec(), ..Default::default() };
        let mut doc = json!({"reasoning_effort": "none", "enable_thinking": true});
        apply_thinking(&mut doc, &uzu, true);
        assert_eq!(doc["reasoning_effort"], "disabled");
        assert!(doc.get("enable_thinking").is_none());
        let mut doc = json!({"reasoning_effort": "max"});
        apply_thinking(&mut doc, &uzu, false);
        assert_eq!(doc["reasoning_effort"], "xhigh");
        let mut doc = json!({"reasoning_effort": "weird"});
        apply_thinking(&mut doc, &uzu, false);
        assert_eq!(doc["reasoning_effort"], "default");
    }

    #[test]
    fn overflow() {
        let markers = Api::default().overflow_markers;
        assert!(is_overflow(&json!({"error": {"message": "Context overflow: too many tokens"}}), &markers));
        assert!(is_overflow(&json!({"error": {"code": "context_length_exceeded", "message": "x"}}), &markers));
        assert!(is_overflow(&json!({"error": "the request exceeds the available context length"}), &markers));
        assert!(!is_overflow(&json!({"error": {"message": "model not found"}}), &markers));
        assert!(!is_overflow(&json!({"choices": []}), &markers));
        let msg = |m: &str| json!({"error": {"message": m}});
        assert_eq!(overflow_tokens(&msg("maximum context length is 65536 tokens, but the rendered prompt has 70123 tokens and leaves no room"), 65536), Some(70123));
        assert_eq!(overflow_tokens(&msg("your messages resulted in 94,605 tokens."), 65536), Some(94605));
        assert_eq!(overflow_tokens(&msg("no numbers here"), 65536), None);
        let llama = json!({"error": {"code": 400, "message": "request (45015 tokens) exceeds the available context size (16384 tokens), try increasing it", "n_ctx": 16384, "n_prompt_tokens": 45015, "type": "exceed_context_size_error"}});
        assert!(is_overflow(&llama, &[]));
        assert_eq!(overflow_tokens(&llama, 16384), Some(45015));
        assert_eq!(overflow_tokens(&msg("request (45015 tokens) exceeds the available context size (16384 tokens)"), 16384), Some(45015));
        assert_eq!(overflow_error(65536, Some(70123), true)["error"]["code"], "context_length_exceeded");
    }

    #[test]
    fn model_rewrite() {
        let line = r#"data: {"id":"x","model":"engine-name","choices":[{"delta":{"content":"the \"model\" word"}}]}"#;
        let out = rewrite_model_field(line, "8gent").unwrap();
        assert!(out.contains(r#""model":"8gent""#) && out.contains(r#"the \"model\" word"#));
        assert_eq!(rewrite_model_field(r#"{"model" : "a\"b"}"#, "z").unwrap(), r#"{"model" : "z"}"#);
        assert!(rewrite_model_field(r#"{"id":1}"#, "z").is_none());
    }
}
