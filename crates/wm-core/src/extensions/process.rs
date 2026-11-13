//! Small process supervisor shared by the node-side drivers.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

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
    pub fn spawn(spec: &ProcessSpec) -> std::io::Result<Self> {
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
            tokio::spawn(read_lines(stdout, tx.clone())),
            tokio::spawn(read_lines(stderr, tx)),
        ];
        Ok(Self {
            child,
            lines: rx,
            readers,
        })
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// Kills the child and waits briefly for it to disappear. The reader tasks
    /// are aborted so a stuck pipe cannot keep the driver alive.
    pub async fn terminate(mut self) {
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(3), self.child.wait()).await;
        for reader in self.readers.drain(..) {
            reader.abort();
        }
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

async fn read_lines<R>(reader: R, tx: mpsc::Sender<String>)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let mut lines = BufReader::new(reader).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if tx.send(line).await.is_err() {
                    break;
                }
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }
}

/// Trims ANSI escapes and collapses whitespace so a log line can be shown as a
/// status message without breaking the panel.
pub fn clip_line(line: &str) -> String {
    let cleaned = strip_ansi(line);
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = collapsed.chars().take(300).collect();
    if collapsed.chars().count() > 300 {
        out.push('…');
    }
    out
}

fn strip_ansi(line: &str) -> String {
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

    #[test]
    fn error_detection() {
        assert!(looks_like_error("2026 ERR failed to connect"));
        assert!(!looks_like_error("INF registered tunnel connection"));
    }
}
