//! What the endpoint has answered: recent requests, totals, prompts that did not fit, and speed per
//! loader and model. The last is what lets 8gent say which loader runs a model fastest on this computer.

use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const RECENT: usize = 200;

#[derive(Clone, Debug, Default, Serialize)]
pub struct RequestRecord {
    pub at: u64,
    pub loader: String,
    pub loader_version: Option<String>,
    pub model: String,
    pub path: String,
    pub stream: bool,
    /// "ok", "error", "overflow" or "cancelled" (the client went away).
    pub finish: String,
    pub status: u16,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    /// Milliseconds until the first piece of the answer (streams only).
    pub first_token_ms: Option<u64>,
    pub duration_ms: u64,
    /// Tokens written per second after the first one arrived.
    pub tokens_per_second: Option<f64>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Overflows {
    pub count: u64,
    pub last_at: Option<u64>,
    pub last_tokens: Option<u64>,
    pub largest_tokens: Option<u64>,
    /// The context window the largest one was measured against.
    pub context: Option<u32>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Totals {
    pub requests: u64,
    pub errors: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Speed {
    pub loader: String,
    pub model: String,
    pub requests: u64,
    pub mean_tokens_per_second: Option<f64>,
    pub mean_first_token_ms: Option<f64>,
    #[serde(skip)]
    tps_sum: f64,
    #[serde(skip)]
    tps_n: u64,
    #[serde(skip)]
    ttft_sum: f64,
    #[serde(skip)]
    ttft_n: u64,
}

#[derive(Default)]
struct Inner {
    recent: VecDeque<RequestRecord>,
    totals: Totals,
    overflows: Overflows,
    speed: BTreeMap<(String, String), Speed>,
    in_flight: u64,
}

pub struct Recorder {
    inner: Mutex<Inner>,
    log: Option<PathBuf>,
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Recorder {
    pub fn new(log: Option<PathBuf>) -> Self {
        Self { inner: Mutex::new(Inner::default()), log }
    }

    pub fn begin(&self) {
        self.inner.lock().unwrap().in_flight += 1;
    }

    pub fn in_flight(&self) -> u64 {
        self.inner.lock().unwrap().in_flight
    }

    pub fn finish(&self, rec: RequestRecord) {
        let mut g = self.inner.lock().unwrap();
        g.in_flight = g.in_flight.saturating_sub(1);
        g.totals.requests += 1;
        if rec.finish == "error" {
            g.totals.errors += 1;
        }
        g.totals.prompt_tokens += rec.prompt_tokens.unwrap_or(0);
        g.totals.completion_tokens += rec.completion_tokens.unwrap_or(0);
        if rec.finish == "ok" {
            let s = g.speed.entry((rec.loader.clone(), rec.model.clone())).or_insert_with(|| Speed { loader: rec.loader.clone(), model: rec.model.clone(), ..Default::default() });
            s.requests += 1;
            // a handful of tokens says nothing about speed
            if let (Some(tps), Some(n)) = (rec.tokens_per_second, rec.completion_tokens) {
                if n >= 16 && tps.is_finite() {
                    s.tps_sum += tps;
                    s.tps_n += 1;
                    s.mean_tokens_per_second = Some(s.tps_sum / s.tps_n as f64);
                }
            }
            if let Some(ms) = rec.first_token_ms {
                s.ttft_sum += ms as f64;
                s.ttft_n += 1;
                s.mean_first_token_ms = Some(s.ttft_sum / s.ttft_n as f64);
            }
        }
        if let Some(path) = &self.log {
            if let (Ok(line), Ok(mut f)) = (serde_json::to_string(&rec), std::fs::OpenOptions::new().create(true).append(true).open(path)) {
                let _ = writeln!(f, "{line}");
            }
        }
        g.recent.push_back(rec);
        while g.recent.len() > RECENT {
            g.recent.pop_front();
        }
    }

    pub fn overflow(&self, context: u32, tokens: Option<u64>) {
        let mut g = self.inner.lock().unwrap();
        let o = &mut g.overflows;
        if o.context != Some(context) {
            // a new window: the old largest prompt no longer says what would fit
            o.largest_tokens = None;
            o.context = Some(context);
        }
        o.count += 1;
        o.last_at = Some(now());
        o.last_tokens = tokens;
        o.largest_tokens = o.largest_tokens.max(tokens);
    }

    pub fn snapshot(&self, recent: usize) -> serde_json::Value {
        let g = self.inner.lock().unwrap();
        let recent: Vec<&RequestRecord> = g.recent.iter().rev().take(recent).collect();
        serde_json::json!({
            "in_flight": g.in_flight,
            "totals": g.totals,
            "overflows": g.overflows,
            "speed": g.speed.values().collect::<Vec<_>>(),
            "recent": recent,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_is_kept_per_loader_and_model() {
        let r = Recorder::new(None);
        for (loader, tps) in [("uzu", 60.0), ("uzu", 80.0), ("llamacpp", 40.0)] {
            r.begin();
            r.finish(RequestRecord { loader: loader.into(), model: "m".into(), finish: "ok".into(), completion_tokens: Some(100),
                tokens_per_second: Some(tps), first_token_ms: Some(200), ..Default::default() });
        }
        let snap = r.snapshot(10);
        let speed = snap["speed"].as_array().unwrap();
        assert_eq!(speed.len(), 2);
        let uzu = speed.iter().find(|s| s["loader"] == "uzu").unwrap();
        assert_eq!(uzu["mean_tokens_per_second"], 70.0);
        assert_eq!(snap["totals"]["requests"], 3);
        assert_eq!(snap["in_flight"], 0);
    }

    #[test]
    fn overflow_largest_resets_with_the_window() {
        let r = Recorder::new(None);
        r.overflow(65536, Some(70000));
        r.overflow(65536, Some(90000));
        r.overflow(131072, Some(140000));
        let snap = r.snapshot(0);
        assert_eq!(snap["overflows"]["count"], 3);
        assert_eq!(snap["overflows"]["largest_tokens"], 140000);
        assert_eq!(snap["overflows"]["context"], 131072);
    }
}
