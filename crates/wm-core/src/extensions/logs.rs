//! Ephemeral, agent-owned extension logs. Never written to disk.
//!
//! RPC `extension_logs {id, limit?}` returns the newest entries in chronological
//! order. Unknown/expired IDs are errors; limit defaults to 100, range 1..=500.
//! Retention: 15 minutes (pruned on access), 500 entries/extension, 128 IDs,
//! 4096 entries globally (~8 MiB message payload), 2048 bytes per message. IDs are opaque (never paths). Output is best effort;
//! applications must not print credentials that cannot be recognized here.
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Value, json};

const MAX_ENTRIES: usize = 500;
const MAX_IDS: usize = 128;
// At most ~8 MiB of message payload across all extensions in one agent.
const MAX_TOTAL_ENTRIES: usize = 4096;
const MAX_MESSAGE: usize = 2048;
const RETENTION: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Default)]
pub(crate) struct ExtensionLogs(Arc<Mutex<HashMap<String, Buffer>>>);
struct Buffer {
    touched: Instant,
    entries: VecDeque<(Instant, Entry)>,
}
#[derive(Clone, Serialize)]
struct Entry {
    timestamp: DateTime<Utc>,
    source: String,
    message: String,
}

#[derive(Clone)]
pub(crate) struct LogSink {
    logs: ExtensionLogs,
    id: String,
}
impl ExtensionLogs {
    pub(crate) fn sink(&self, id: &str) -> LogSink {
        LogSink { logs: self.clone(), id: id.into() }
    }

    pub(crate) fn read(&self, params: Value) -> Result<Value, String> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Params { id: String, limit: Option<usize> }
        let params: Params = serde_json::from_value(params).map_err(|_| "informe {id, limit?}".to_string())?;
        if params.id.is_empty() || params.id.len() > 256 || params.id.chars().any(char::is_control) {
            return Err("id inválido".into());
        }
        let limit = params.limit.unwrap_or(100);
        if !(1..=MAX_ENTRIES).contains(&limit) {
            return Err("limit deve estar entre 1 e 500".into());
        }
        let mut buffers = self.0.lock().map_err(|_| "logs indisponíveis")?;
        prune(&mut buffers, Instant::now());
        let buffer = buffers.get(&params.id).ok_or("extensão sem logs retidos")?;
        let entries: Vec<_> = buffer.entries.iter().skip(buffer.entries.len().saturating_sub(limit)).map(|(_, entry)| entry.clone()).collect();
        Ok(json!({"id": params.id, "entries": entries, "retention_secs": RETENTION.as_secs(), "max_entries": MAX_ENTRIES, "max_message_bytes": MAX_MESSAGE}))
    }
}
fn prune(buffers: &mut HashMap<String, Buffer>, now: Instant) {
    for buffer in buffers.values_mut() {
        while buffer.entries.front().is_some_and(|(at, _)| now.duration_since(*at) >= RETENTION) {
            buffer.entries.pop_front();
        }
    }
    buffers.retain(|_, buffer| !buffer.entries.is_empty());
}
impl LogSink {
    pub(crate) fn record(&self, source: &str, message: &str) {
        let message = sanitize(message);
        if message.is_empty() || self.id.is_empty() || self.id.len() > 256 { return; }
        let Ok(mut buffers) = self.logs.0.lock() else { return; };
        let now = Instant::now();
        prune(&mut buffers, now);
        let total: usize = buffers.values().map(|b| b.entries.len()).sum();
        if total >= MAX_TOTAL_ENTRIES {
            // Evict the oldest inactive ID as a unit, retaining this stream's
            // useful contiguous tail. Per-extension bounds handle a sole ID.
            if let Some(oldest) = buffers.iter().filter(|(id, _)| *id != &self.id)
                .min_by_key(|(_, b)| b.touched).map(|(id, _)| id.clone()) {
                buffers.remove(&oldest);
            }
        }
        if !buffers.contains_key(&self.id) && buffers.len() >= MAX_IDS {
            if let Some(oldest) = buffers.iter().min_by_key(|(_, b)| b.touched).map(|(id, _)| id.clone()) {
                buffers.remove(&oldest);
            }
        }
        let buffer = buffers.entry(self.id.clone()).or_insert_with(|| Buffer { touched: now, entries: VecDeque::new() });
        buffer.touched = now;
        if buffer.entries.len() == MAX_ENTRIES { buffer.entries.pop_front(); }
        buffer.entries.push_back((now, Entry { timestamp: Utc::now(), source: source.into(), message }));
    }
}

/// Fail closed on structured config/credential markers. Redact BEFORE clipping:
/// cutting first could retain a secret but lose its marker. Unknown application
/// secrets printed without a recognizable marker remain the author's concern.
pub(crate) fn sanitize(input: &str) -> String {
    let cleaned = super::process::strip_ansi(input);
    let cleaned: String = cleaned.chars().filter(|c| !c.is_control() || c.is_whitespace()).collect();
    let lower = cleaned.to_ascii_lowercase();
    if (cleaned.contains('{') || (cleaned.trim_start().starts_with('[') && !cleaned.starts_with("[REDACTED]") && !cleaned.starts_with("[output line omitted:") && cleaned != "[REDACTED: credential/config output]")) || ["token", "password", "passwd", "secret", "authorization", "bearer ", "cookie", "credential", "wm_extension_config", "api_key", "api-key", "private_key", "private-key", "auth_key", "auth-key", "access_key", "access-key", "--key"].iter().any(|marker| lower.contains(marker)) {
        return "[REDACTED: credential/config output]".into();
    }
    // WM auth token prefixes, including tokens embedded in punctuation/URLs.
    let mut out = String::new();
    let mut rest = cleaned.as_str();
    while !rest.is_empty() {
        if ["wmn_", "wmj_", "wms_", "wmsess_", "wmcsrf_", "wmc_", "wmgui_"].iter().any(|p| rest.starts_with(p)) {
            let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))).unwrap_or(rest.len());
            out.push_str("[REDACTED]");
            rest = &rest[end..];
        } else {
            let c = rest.chars().next().unwrap();
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    let mut result = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if result.len() > MAX_MESSAGE {
        let mut end = MAX_MESSAGE - '…'.len_utf8();
        while !result.is_char_boundary(end) { end -= 1; }
        result.truncate(end);
        result.push('…');
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn limits_isolation_and_ids() {
        let logs = ExtensionLogs::default();
        for n in 0..600 { logs.sink("one").record("stdout", &n.to_string()); }
        logs.sink("two").record("stderr", "isolated");
        let all = logs.read(json!({"id":"one", "limit":500})).unwrap();
        assert_eq!(all["entries"].as_array().unwrap().len(), 500);
        assert_eq!(all["entries"][0]["message"], "100");
        assert_eq!(logs.read(json!({"id":"one", "limit":1})).unwrap()["entries"][0]["message"], "599");
        assert!(ExtensionLogs::default().read(json!({"id":"one"})).is_err());
        for params in [json!({}), json!({"id":""}), json!({"id":1}), json!({"id":"one","limit":0}), json!({"id":"one","limit":501}), json!({"id":"one","limit":-1}), json!({"id":"unknown"})] { assert!(logs.read(params).is_err()); }
        for n in 0..200 { logs.sink(&format!("id{n}")).record("lifecycle", "created"); }
        assert_eq!(logs.0.lock().unwrap().len(), MAX_IDS);
    }
    #[test]
    fn total_memory_is_bounded() {
        let logs = ExtensionLogs::default();
        for id in 0..12 {
            for _ in 0..MAX_ENTRIES {
                logs.sink(&id.to_string()).record("stdout", "payload");
            }
        }
        let buffers = logs.0.lock().unwrap();
        assert!(buffers.values().map(|b| b.entries.len()).sum::<usize>() <= MAX_TOTAL_ENTRIES);
        assert!(!buffers.contains_key("0"));
        assert_eq!(buffers["11"].entries.len(), MAX_ENTRIES);
    }

    #[test]
    fn retention_and_sanitization() {
        let logs = ExtensionLogs::default();
        logs.sink("old").record("stdout", "expired");
        logs.0.lock().unwrap().get_mut("old").unwrap().entries[0].0 = Instant::now() - RETENTION;
        assert!(logs.read(json!({"id":"old"})).is_err());
        for raw in ["--token=abc", "PASSWORD = abc", "Authorization: Bearer abc", "{\"config\":\"abc\"}", "WM_EXTENSION_CONFIG=abc"] {
            assert!(!sanitize(raw).contains("abc"));
        }
        assert_eq!(sanitize("ok (wmn_abc-123) wmj_xyz wmsess_foo wmc_connect wmgui_capability"), "ok ([REDACTED]) [REDACTED] [REDACTED] [REDACTED] [REDACTED]");
        assert_eq!(sanitize("\x1b[31mhello\x1b[0m\0\r world"), "hello world");
        assert!(sanitize(&"é".repeat(5000)).len() <= MAX_MESSAGE);
        assert!(!sanitize(&format!("{} --token abc", "x".repeat(5000))).contains('x'));
    }
}
