//! Durable TUI state.
//!
//! The agent owns the network state (`agent-state.json`); the TUI only stores
//! presentation preferences, the last snapshot it saw and an append-only event
//! log. Nothing here contains credentials: join codes and tokens are redacted
//! before they reach disk.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use wm_proto::{AgentStatus, ExtensionStatus, PeerStatus, ShareInfo};

pub const STATE_FILE: &str = "tui-state.json";
pub const EVENTS_FILE: &str = "tui-events.jsonl";
pub const AGENT_LOG: &str = "agent.log";
pub const STATE_VERSION: u32 = 1;

const EVENTS_ROTATE_BYTES: u64 = 512 * 1024;
const EVENTS_KEEP_LINES: usize = 500;

fn default_udp_port() -> u16 {
    wm_proto::DEFAULT_UDP_PORT
}

fn default_mtu() -> u16 {
    wm_proto::DEFAULT_MTU
}

/// Options used when the TUI starts the agent. Persisted so reopening the TUI
/// restarts the node exactly as it was left.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentSettings {
    pub udp_port: u16,
    pub mtu: u16,
    pub force_relay: bool,
    /// Comma-separated `porta` or `overlay:local` specs, as typed in the form.
    pub expose: String,
    pub tun_name: String,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            udp_port: default_udp_port(),
            mtu: default_mtu(),
            force_relay: false,
            expose: String::new(),
            tun_name: String::new(),
        }
    }
}

impl AgentSettings {
    /// Parses and validates the expose field. Empty means "no extra expose".
    pub fn expose_specs(&self) -> Result<Vec<String>, String> {
        let mut out = Vec::new();
        for raw in self.expose.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let (overlay, local) = match raw.split_once(':') {
                Some((a, b)) => (a.trim(), b.trim()),
                None => (raw, raw),
            };
            let overlay: u16 = overlay
                .parse()
                .map_err(|_| format!("porta overlay inválida em '{raw}'"))?;
            let local: u16 = local
                .parse()
                .map_err(|_| format!("porta local inválida em '{raw}'"))?;
            if overlay == 0 || local == 0 {
                return Err(format!("porta 0 não é válida em '{raw}'"));
            }
            out.push(if overlay == local {
                overlay.to_string()
            } else {
                format!("{overlay}:{local}")
            });
        }
        Ok(out)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedJob {
    pub pid: u32,
    pub label: String,
    pub log: PathBuf,
    #[serde(default)]
    pub started_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoticeRecord {
    pub at: DateTime<Utc>,
    pub level: String,
    pub text: String,
}

/// Last snapshot shown by the TUI. It lets a reopened TUI show the mesh as it
/// was left even while the agent is still booting (or stays stopped).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotCache {
    pub at: DateTime<Utc>,
    #[serde(default)]
    pub status: Option<AgentStatus>,
    #[serde(default)]
    pub peers: Vec<PeerStatus>,
    #[serde(default)]
    pub shares: Vec<ShareInfo>,
    #[serde(default)]
    pub extensions: Vec<ExtensionStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PersistedState {
    pub version: u32,
    pub tab: usize,
    pub peer_index: usize,
    pub share_index: usize,
    pub auto_start: bool,
    pub settings: AgentSettings,
    /// Command-line history (redacted).
    pub history: Vec<String>,
    /// Recent notices, so a new session can show what the last one did.
    pub notices: Vec<NoticeRecord>,
    /// Detached jobs that were still running when the TUI was closed.
    pub jobs: Vec<PersistedJob>,
    pub cache: Option<SnapshotCache>,
    pub last_session_start: Option<DateTime<Utc>>,
    pub last_session_end: Option<DateTime<Utc>>,
}

impl Default for PersistedState {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            tab: 0,
            peer_index: 0,
            share_index: 0,
            auto_start: true,
            settings: AgentSettings::default(),
            history: Vec::new(),
            notices: Vec::new(),
            jobs: Vec::new(),
            cache: None,
            last_session_start: None,
            last_session_end: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecord {
    pub ts: DateTime<Utc>,
    pub level: String,
    pub text: String,
}

/// The subset of `agent-state.json` the TUI needs. Read-only: the agent owns
/// the file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EnrolledNode {
    #[serde(default)]
    pub node_name: String,
    #[serde(default)]
    pub overlay_ip: String,
    #[serde(default)]
    pub overlay_cidr: String,
    #[serde(default)]
    pub control_url: String,
    #[serde(default)]
    pub network_name: String,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    /// Presentation identity; empty means "not configured".
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub about: String,
}

pub fn state_path(root: &Path) -> PathBuf {
    root.join(STATE_FILE)
}

pub fn events_path(root: &Path) -> PathBuf {
    root.join(EVENTS_FILE)
}

pub fn agent_log_path(root: &Path) -> PathBuf {
    root.join(AGENT_LOG)
}

/// Creates the data directory when missing and restricts it to the operator.
pub fn ensure_dir(root: &Path) -> io::Result<()> {
    if root.exists() {
        return Ok(());
    }
    fs::create_dir_all(root)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(root, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

pub fn load_state(root: &Path) -> (PersistedState, Option<String>) {
    match fs::read_to_string(state_path(root)) {
        Ok(text) => match serde_json::from_str::<PersistedState>(&text) {
            Ok(mut state) => {
                if state.version > STATE_VERSION {
                    return (
                        PersistedState::default(),
                        Some(format!(
                            "estado da TUI de uma versão mais nova (v{}); usando valores padrão",
                            state.version
                        )),
                    );
                }
                state.version = STATE_VERSION;
                (state, None)
            }
            Err(err) => (
                PersistedState::default(),
                Some(format!("estado da TUI ilegível: {err}")),
            ),
        },
        Err(err) if err.kind() == io::ErrorKind::NotFound => (PersistedState::default(), None),
        Err(err) => (
            PersistedState::default(),
            Some(format!("não foi possível ler o estado da TUI: {err}")),
        ),
    }
}

pub fn save_state(root: &Path, state: &PersistedState) -> io::Result<()> {
    ensure_dir(root)?;
    let path = state_path(root);
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_vec_pretty(state)?;
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(&text)?;
        file.sync_all().ok();
    }
    fs::rename(&tmp, &path)
}

pub fn load_enrolled(root: &Path) -> Result<Option<EnrolledNode>, String> {
    let path = root.join("agent-state.json");
    match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|err| format!("estado do agente ilegível ({}): {err}", path.display())),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("não foi possível ler {}: {err}", path.display())),
    }
}

pub fn append_event(root: &Path, level: &str, text: &str) {
    let record = EventRecord {
        ts: Utc::now(),
        level: level.to_string(),
        text: sanitize(&redact(text)),
    };
    let Ok(mut line) = serde_json::to_vec(&record) else {
        return;
    };
    line.push(b'\n');
    let _ = ensure_dir(root);
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(events_path(root))
    {
        let _ = file.write_all(&line);
        let too_big = file
            .metadata()
            .map(|meta| meta.len() > EVENTS_ROTATE_BYTES)
            .unwrap_or(false);
        drop(file);
        if too_big {
            trim_events(root);
        }
    }
}

/// Keeps the event log bounded. Called once per TUI start.
pub fn trim_events(root: &Path) {
    let path = events_path(root);
    let Ok(meta) = fs::metadata(&path) else {
        return;
    };
    if meta.len() <= EVENTS_ROTATE_BYTES {
        return;
    }
    let Ok(text) = fs::read_to_string(&path) else {
        return;
    };
    let keep: Vec<&str> = text
        .lines()
        .rev()
        .take(EVENTS_KEEP_LINES)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let body = keep.join("\n") + "\n";
    let tmp = path.with_extension("jsonl.tmp");
    if fs::write(&tmp, body).is_ok() {
        let _ = fs::rename(&tmp, &path);
    }
}

/// Reads the most recent `max` events. Corrupt lines are skipped, never fatal.
pub fn read_events(root: &Path, max: usize) -> Vec<EventRecord> {
    let Ok(text) = fs::read_to_string(events_path(root)) else {
        return Vec::new();
    };
    let mut out: Vec<EventRecord> = text
        .lines()
        .rev()
        .take(max)
        .filter_map(|line| serde_json::from_str::<EventRecord>(line).ok())
        .map(|mut record| {
            record.text = sanitize(&record.text);
            record
        })
        .collect();
    out.reverse();
    out
}

const SECRET_PREFIXES: [&str; 4] = ["wmj_", "wmc_", "wms_", "wmgui_"];
const SECRET_FLAGS: [&str; 3] = ["--token", "--admin-password", "--code"];

fn token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~')
}

/// Replaces opaque credentials by `<oculto>` while keeping the rest readable.
pub fn redact(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        let prefix = SECRET_PREFIXES
            .iter()
            .find(|prefix| text[index..].starts_with(**prefix));
        if let Some(prefix) = prefix {
            let mut end = index + prefix.len();
            while end < bytes.len() && token_byte(bytes[end]) {
                end += 1;
            }
            if end.saturating_sub(index + prefix.len()) >= 12 {
                out.push_str(prefix);
                out.push_str("<oculto>");
                index = end;
                continue;
            }
        }
        let ch = text[index..].chars().next().expect("char boundary");
        out.push(ch);
        index += ch.len_utf8();
    }
    out
}

/// Redacts flags whose value is a credential, then runs [`redact`].
pub fn redact_input(input: &str) -> String {
    let mut text = input.to_string();
    for flag in SECRET_FLAGS {
        let mut search = 0;
        while let Some(found) = text[search..].find(flag) {
            let start = search + found;
            let after = start + flag.len();
            let rest = &text[after..];
            let spaces = rest.len() - rest.trim_start_matches([' ', '\t']).len();
            let value_start = if rest.starts_with('=') {
                after + 1
            } else if spaces > 0 {
                after + spaces
            } else {
                search = after;
                continue;
            };
            let value_len = text[value_start..]
                .find(char::is_whitespace)
                .unwrap_or(text.len() - value_start);
            let value_end = value_start + value_len;
            if value_len > 0 && !text[value_start..value_end].starts_with('<') {
                text.replace_range(value_start..value_end, "<oculto>");
                search = value_start + "<oculto>".len();
            } else {
                search = value_end;
            }
        }
    }
    redact(&text)
}

/// Removes ANSI escape sequences and control characters before text reaches
/// the terminal or the event log. Agent logs and CLI output can contain color
/// codes; rendering them raw could corrupt the TUI.
pub fn sanitize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            match chars.next() {
                Some('[') => {
                    // CSI: ESC [ ... final byte in @..~
                    while let Some(next) = chars.next() {
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    // OSC: ESC ] ... BEL or ESC \
                    while let Some(next) = chars.next() {
                        if next == '\u{7}' {
                            break;
                        }
                        if next == '\u{1b}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                Some('(' | ')' | '#' | '%') => {
                    let _ = chars.next();
                }
                Some(_) | None => {}
            }
            continue;
        }
        if ch == '\n' {
            out.push(ch);
        } else if ch == '\t' {
            out.push(' ');
        } else if !ch.is_control() {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "wm-tui-persist-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn state_roundtrip_keeps_preferences_and_cache() {
        let root = temp_root("roundtrip");
        let mut state = PersistedState {
            tab: 4,
            auto_start: false,
            peer_index: 3,
            ..PersistedState::default()
        };
        state.settings.udp_port = 12345;
        state.settings.expose = "8421:3000".into();
        state.history.push("status".into());
        save_state(&root, &state).unwrap();
        let (loaded, warning) = load_state(&root);
        assert!(warning.is_none());
        assert_eq!(loaded.tab, 4);
        assert!(!loaded.auto_start);
        assert_eq!(loaded.peer_index, 3);
        assert_eq!(loaded.settings.udp_port, 12345);
        assert_eq!(loaded.settings.expose, "8421:3000");
        assert_eq!(loaded.history, vec!["status"]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(state_path(&root)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_state_is_not_fatal() {
        let root = temp_root("corrupt");
        fs::write(state_path(&root), "{not json").unwrap();
        let (state, warning) = load_state(&root);
        assert!(warning.is_some());
        assert_eq!(state.version, STATE_VERSION);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn events_survive_and_secrets_are_redacted() {
        let root = temp_root("events");
        append_event(&root, "info", "convite wmj_abcdefghijklmnop recebido");
        let events = read_events(&root, 10);
        assert_eq!(events.len(), 1);
        assert!(events[0].text.contains("wmj_<oculto>"));
        assert!(!events[0].text.contains("abcdefghijklmnop"));
        let raw = fs::read_to_string(events_path(&root)).unwrap();
        assert!(!raw.contains("abcdefghijklmnop"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn input_redaction_hides_flag_values() {
        let line = "join https://mesh.test --token wmj_short --name box";
        assert_eq!(
            redact_input(line),
            "join https://mesh.test --token <oculto> --name box"
        );
        let line = "host --admin-password correct horse";
        assert_eq!(redact_input(line), "host --admin-password <oculto> horse");
        let code = "wmc_eyJ1cmwiOiJodHRwczovL21lc2guZXhhbXBsZSJ9";
        let out = redact_input(code);
        assert!(out.starts_with("wmc_<oculto>"), "{out}");
    }

    #[test]
    fn sanitize_strips_ansi_and_controls() {
        assert_eq!(sanitize("\u{1b}[32mINFO\u{1b}[0m ok"), "INFO ok");
        assert_eq!(sanitize("a\rb\u{0}c"), "abc");
        assert_eq!(sanitize("linha 1\nlinha 2"), "linha 1\nlinha 2");
        assert_eq!(sanitize("tab\taqui"), "tab aqui");
        assert_eq!(sanitize("\u{1b}]0;t\u{7}resto"), "resto");
    }

    #[test]
    fn expose_specs_validate_ports() {
        let mut settings = AgentSettings::default();
        settings.expose = "8421, 9000:9001".into();
        assert_eq!(settings.expose_specs().unwrap(), vec!["8421", "9000:9001"]);
        settings.expose = "99999".into();
        assert!(settings.expose_specs().is_err());
        settings.expose = "abc".into();
        assert!(settings.expose_specs().is_err());
    }

    #[test]
    fn trim_events_keeps_tail() {
        let root = temp_root("trim");
        let total = EVENTS_KEEP_LINES * 20;
        let mut body = String::new();
        for index in 0..total {
            body.push_str(&format!(
                "{{\"ts\":\"2026-01-01T00:00:00Z\",\"level\":\"info\",\"text\":\"line {index} com um texto maior para passar do limite de rotação\"}}\n"
            ));
        }
        fs::write(events_path(&root), body).unwrap();
        trim_events(&root);
        let text = fs::read_to_string(events_path(&root)).unwrap();
        assert!(text.lines().count() <= EVENTS_KEEP_LINES + 1);
        assert!(text.contains(&format!("line {} ", total - 1)));
        assert!(!text.contains("line 0 "));
        fs::remove_dir_all(root).unwrap();
    }
}
