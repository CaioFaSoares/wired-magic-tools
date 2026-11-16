//! Small process supervisor shared by the node-side drivers.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use super::logs::LogSink;

#[derive(Debug, Clone)]
pub struct ProcessSpec {
    pub program: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: Option<PathBuf>,
}

impl ProcessSpec {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
        }
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }
}

/// A spawned sidecar with its output merged into one line stream. The child is
/// killed when this value is dropped, so aborting a driver task cannot leak a
/// process.
pub struct ManagedProcess {
    pub child: Child,
    pub lines: mpsc::Receiver<String>,
    readers: Vec<JoinHandle<()>>,
}

impl ManagedProcess {
    pub(crate) fn spawn_logged(spec: &ProcessSpec, logs: Option<LogSink>, prefix: &str) -> std::io::Result<Self> {
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for (key, value) in &spec.env {
            command.env(key, value);
        }
        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd);
        }
        let mut child = command.spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("stdout não capturado"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| std::io::Error::other("stderr não capturado"))?;
        let (tx, rx) = mpsc::channel(256);
        let readers = vec![
            tokio::spawn(read_lines(stdout, tx.clone(), logs.clone(), format!("{prefix}stdout"))),
            tokio::spawn(read_lines(stderr, tx, logs, format!("{prefix}stderr"))),
        ];
        Ok(Self {
            child,
            lines: rx,
            readers,
        })
    }

    /// Drain trailing output, with a deadline for pipes inherited by descendants.
    pub(crate) async fn finish_output(&mut self) {
        for mut reader in self.readers.drain(..) {
            if tokio::time::timeout(Duration::from_millis(250), &mut reader).await.is_err() {
                reader.abort();
                let _ = reader.await;
            }
        }
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// Kills the child and waits briefly for it to disappear. The reader tasks
    /// are aborted so a stuck pipe cannot keep the driver alive.
    pub async fn terminate(mut self) {
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(3), self.child.wait()).await;
        self.finish_output().await;
    }
}

impl Drop for ManagedProcess {
    fn drop(&mut self) {
        for reader in self.readers.drain(..) {
            reader.abort();
        }
        let _ = self.child.start_kill();
    }
}

// Never allocate an unbounded line. Oversized lines are discarded in full:
// truncating before redaction could hide a credential marker at the end.
async fn read_lines<R>(reader: R, tx: mpsc::Sender<String>, logs: Option<LogSink>, source: String)
where R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    const MAX_INPUT: usize = 8192;
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    let mut oversized = false;
    loop {
        let chunk = match reader.fill_buf().await { Ok(chunk) => chunk, Err(_) => break };
        if chunk.is_empty() {
            if !line.is_empty() || oversized { emit_line(&line, oversized, &tx, &logs, &source); }
            break;
        }
        let end = chunk.iter().position(|b| *b == b'\n');
        let count = end.map_or(chunk.len(), |end| end + 1);
        if !oversized {
            if line.len() + count > MAX_INPUT { oversized = true; line.clear(); }
            else { line.extend_from_slice(&chunk[..count]); }
        }
        reader.consume(count);
        if end.is_some() {
            emit_line(&line, oversized, &tx, &logs, &source);
            line.clear();
            oversized = false;
        }
    }
}
fn emit_line(bytes: &[u8], oversized: bool, tx: &mpsc::Sender<String>, logs: &Option<LogSink>, source: &str) {
    let line = if oversized { "[output line omitted: exceeds 8192 bytes]".into() }
        else { String::from_utf8_lossy(bytes).into_owned() };
    if let Some(logs) = logs { logs.record(source, &line); }
    // Output capture must not block the child, even after wait() wins a select
    // or the consumer stops. This notification stream is best effort.
    let _ = tx.try_send(line);
}

/// Trims ANSI escapes and collapses whitespace so a log line can be shown as a
/// status message without breaking the panel.
pub fn clip_line(line: &str) -> String {
    let cleaned = super::logs::sanitize(line);
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = collapsed.chars().take(300).collect();
    if collapsed.chars().count() > 300 {
        out.push('…');
    }
    out
}

pub(crate) fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
                continue;
            }
        }
        out.push(c);
    }
    out
}

pub fn looks_like_error(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains(" err ")
        || lower.contains("error")
        || lower.contains("failed")
        || lower.contains("fatal")
        || lower.contains("unable to")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clips_and_strips() {
        let line = "\u{1b}[36mINF\u{1b}[0m  hello   world";
        assert_eq!(clip_line(line), "INF hello world");
        assert!(clip_line(&"x".repeat(400)).ends_with('…'));
    }

    #[tokio::test]
    async fn mock_capture_bounds_redacts_and_does_not_block_on_notifications() {
        use tokio::io::AsyncWriteExt;
        use super::super::logs::ExtensionLogs;
        use serde_json::json;
        let logs = ExtensionLogs::default();
        let (mut writer, reader) = tokio::io::duplex(64);
        let (tx, _unread) = mpsc::channel(1);
        let capture = tokio::spawn(read_lines(reader, tx, Some(logs.sink("mock")), "hook.activate.stderr".into()));
        let body = format!("--token supersecret\n{} password late-secret\n{}\nlast wmn_123", "é".repeat(10000), "normal\n".repeat(600));
        writer.write_all(body.as_bytes()).await.unwrap();
        writer.shutdown().await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), capture).await.unwrap().unwrap();
        let result = logs.read(json!({"id":"mock", "limit":500})).unwrap();
        let entries = result["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 500);
        assert_eq!(entries.last().unwrap()["message"], "last [REDACTED]");
        assert_eq!(entries[0]["source"], "hook.activate.stderr");
        assert!(!result.to_string().contains("secret"));
    }

    #[tokio::test]
    async fn mock_capture_discards_whole_oversized_lines_and_invalid_utf8() {
        use tokio::io::AsyncWriteExt;
        use super::super::logs::ExtensionLogs;
        use serde_json::json;
        let logs = ExtensionLogs::default();
        let (mut writer, reader) = tokio::io::duplex(64);
        let (tx, _rx) = mpsc::channel(1);
        let capture = tokio::spawn(read_lines(reader, tx, Some(logs.sink("mock")), "exec.stdout".into()));
        writer.write_all(&vec![b'x'; 100000]).await.unwrap();
        writer.write_all(b" --token secret\nvalid\xff\n").await.unwrap();
        writer.shutdown().await.unwrap();
        capture.await.unwrap();
        let result = logs.read(json!({"id":"mock"})).unwrap();
        assert_eq!(result["entries"][0]["message"], "[output line omitted: exceeds 8192 bytes]");
        assert_eq!(result["entries"].as_array().unwrap().len(), 2);
        assert!(!result.to_string().contains("secret"));
    }

    #[test]
    fn error_detection() {
        assert!(looks_like_error("2026 ERR failed to connect"));
        assert!(!looks_like_error("INF registered tunnel connection"));
    }
}
