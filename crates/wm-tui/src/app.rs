//! TUI application state: onboarding, live snapshot, forms, notifications and
//! what gets persisted between sessions.

use std::path::PathBuf;
use std::time::Instant;

use chrono::{DateTime, Utc};
use ratatui::style::Color;
use ratatui::widgets::TableState;
use uuid::Uuid;
use wm_proto::{AgentStatus, ExtensionStatus, PeerStatus, ShareInfo};

use crate::agent;
use crate::persist::{
    self, AgentSettings, EnrolledNode, NoticeRecord, PersistedJob, PersistedState, SnapshotCache,
};

pub const TABS: [&str; 8] = [
    "Início",
    "Peers",
    "Shares",
    "Mapa",
    "Agente",
    "Serviços",
    "Console",
    "Ajuda",
];
pub const TAB_STATUS: usize = 0;
pub const TAB_PEERS: usize = 1;
pub const TAB_SHARES: usize = 2;
pub const TAB_TOPOLOGY: usize = 3;
pub const TAB_AGENT: usize = 4;
pub const TAB_SERVICES: usize = 5;
pub const TAB_CONSOLE: usize = 6;
pub const TAB_HELP: usize = 7;

pub const MAX_CONSOLE_LINES: usize = 3_000;
pub const MAX_HISTORY: usize = 100;
pub const MAX_NOTICES: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Good,
    Warn,
    Error,
    /// Raw stdout of a CLI child.
    Output,
}

impl Level {
    pub fn color(self) -> Color {
        match self {
            Level::Info => Color::Gray,
            Level::Good => Color::Green,
            Level::Warn => Color::Yellow,
            Level::Error => Color::Red,
            Level::Output => Color::DarkGray,
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            Level::Info => "·",
            Level::Good => "✓",
            Level::Warn => "!",
            Level::Error => "✗",
            Level::Output => " ",
        }
    }

    pub fn persist_name(self) -> &'static str {
        match self {
            Level::Info => "info",
            Level::Good => "ok",
            Level::Warn => "warn",
            Level::Error => "error",
            Level::Output => "out",
        }
    }

    pub fn from_persisted(value: &str) -> Self {
        match value {
            "ok" => Level::Good,
            "warn" => Level::Warn,
            "error" => Level::Error,
            "out" => Level::Output,
            _ => Level::Info,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Notice {
    pub at: DateTime<Utc>,
    pub level: Level,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct ConsoleLine {
    pub level: Level,
    pub text: String,
    pub previous: bool,
}

#[derive(Debug, Clone)]
pub struct Job {
    pub pid: u32,
    pub label: String,
    pub log: PathBuf,
    pub stopping: bool,
}

#[derive(Debug, Default)]
pub struct AgentView {
    pub starting: Option<Instant>,
    pub stopping: Option<Instant>,
    pub pid: Option<u32>,
    /// True when this TUI session started the process.
    pub managed: bool,
    pub log: Vec<String>,
    pub last_error: Option<String>,
    /// Set when a stop was confirmed, so the later process exit is not
    /// reported twice as an unexpected crash.
    pub last_stop_pid: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Digits,
    Choice,
    Bool,
}

#[derive(Debug, Clone)]
pub struct Field {
    pub label: &'static str,
    pub value: String,
    pub kind: FieldKind,
    pub options: &'static [&'static str],
    pub hint: &'static str,
    pub max: usize,
}

impl Field {
    pub fn text(label: &'static str, value: &str, max: usize, hint: &'static str) -> Self {
        Self {
            label,
            value: value.to_string(),
            kind: FieldKind::Text,
            options: &[],
            hint,
            max,
        }
    }

    pub fn digits(label: &'static str, value: &str, hint: &'static str) -> Self {
        Self {
            label,
            value: value.to_string(),
            kind: FieldKind::Digits,
            options: &[],
            hint,
            max: 5,
        }
    }

    pub fn choice(
        label: &'static str,
        value: &str,
        options: &'static [&'static str],
        hint: &'static str,
    ) -> Self {
        Self {
            label,
            value: value.to_string(),
            kind: FieldKind::Choice,
            options,
            hint,
            max: 0,
        }
    }

    pub fn boolean(label: &'static str, value: bool, hint: &'static str) -> Self {
        Self {
            label,
            value: if value { "sim" } else { "não" }.into(),
            kind: FieldKind::Bool,
            options: &["não", "sim"],
            hint,
            max: 0,
        }
    }

    pub fn truthy(&self) -> bool {
        self.value == "sim"
    }

    pub fn cycle(&mut self, forward: bool) {
        let options = self.options;
        if options.is_empty() {
            return;
        }
        let index = options
            .iter()
            .position(|option| *option == self.value)
            .unwrap_or(0) as isize;
        let delta = if forward { 1 } else { -1 };
        let next = (index + delta).rem_euclid(options.len() as isize) as usize;
        self.value = options[next].to_string();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormAction {
    Join,
    NewShare,
    RenameShare(Uuid),
    RenameNode,
    AgentSettings,
    ProbePort,
}

#[derive(Debug)]
pub struct Form {
    pub title: &'static str,
    pub action: FormAction,
    pub fields: Vec<Field>,
    pub focus: usize,
    pub error: Option<String>,
    pub note: Option<&'static str>,
}

impl Form {
    pub fn new(title: &'static str, action: FormAction, fields: Vec<Field>) -> Self {
        Self {
            title,
            action,
            fields,
            focus: 0,
            error: None,
            note: None,
        }
    }

    pub fn with_note(mut self, note: &'static str) -> Self {
        self.note = Some(note);
        self
    }

    pub fn focused(&self) -> Option<&Field> {
        self.fields.get(self.focus)
    }

    pub fn next_field(&mut self) {
        self.error = None;
        if !self.fields.is_empty() {
            self.focus = (self.focus + 1) % self.fields.len();
        }
    }

    pub fn previous_field(&mut self) {
        self.error = None;
        if !self.fields.is_empty() {
            self.focus = (self.focus + self.fields.len() - 1) % self.fields.len();
        }
    }

    pub fn value(&self, index: usize) -> &str {
        self.fields
            .get(index)
            .map(|field| field.value.trim())
            .unwrap_or("")
    }

    pub fn value_owned(&self, index: usize) -> String {
        self.value(index).to_string()
    }

    pub fn bool_at(&self, index: usize) -> bool {
        self.fields.get(index).is_some_and(Field::truthy)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmAction {
    StopAgent,
    RestartAgent,
    StopJob(u32),
    InstallService,
    UninstallService,
}

#[derive(Debug)]
pub struct Confirm {
    pub title: String,
    pub lines: Vec<String>,
    pub action: ConfirmAction,
}

#[derive(Debug)]
pub struct Input {
    pub value: String,
    pub history_index: Option<usize>,
    pub error: Option<String>,
}

pub struct App {
    pub root: PathBuf,
    pub exe: PathBuf,
    pub socket: PathBuf,
    pub log_path: PathBuf,
    pub tab: usize,
    pub status: Option<AgentStatus>,
    pub peers: Vec<PeerStatus>,
    pub shares: Vec<ShareInfo>,
    pub extensions: Vec<ExtensionStatus>,
    pub node: Option<EnrolledNode>,
    /// Why `agent-state.json` could not be read (permissions/parse), if so.
    pub node_error: Option<String>,
    pub live: bool,
    pub error: Option<String>,
    pub updated: Option<Instant>,
    pub cached_at: Option<DateTime<Utc>>,
    pub table: TableState,
    pub shares_table: TableState,
    pub jobs_table: TableState,
    pub jobs: Vec<Job>,
    pub agent: AgentView,
    pub settings: AgentSettings,
    pub auto_start: bool,
    pub console: Vec<ConsoleLine>,
    pub console_scroll: usize,
    pub help_scroll: usize,
    pub notices: Vec<Notice>,
    pub history: Vec<String>,
    pub input: Option<Input>,
    pub form: Option<Form>,
    pub confirm: Option<Confirm>,
    pub delete_armed: Option<(Uuid, Instant)>,
    pub restart_pending: bool,
    pub children: Vec<u32>,
    pub last_session_end: Option<DateTime<Utc>>,
    pub session_started: DateTime<Utc>,
    pub quit: bool,
    pub dirty: bool,
    pub last_save: Instant,
    /// Last save failure, to avoid warning on every periodic retry.
    pub save_error: Option<String>,
}

impl App {
    pub fn load(root: PathBuf) -> Self {
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("wm"));
        let socket = root.join("agent.sock");
        let log_path = persist::agent_log_path(&root);
        let (state, warning) = persist::load_state(&root);
        persist::trim_events(&root);
        let peer_index = state.peer_index;
        let share_index = state.share_index;

        let mut app = Self {
            root,
            exe,
            socket: socket.clone(),
            log_path,
            tab: state.tab.min(TABS.len() - 1),
            status: None,
            peers: Vec::new(),
            shares: Vec::new(),
            extensions: Vec::new(),
            node: None,
            node_error: None,
            live: false,
            error: None,
            updated: None,
            cached_at: None,
            table: TableState::default(),
            shares_table: TableState::default(),
            jobs_table: TableState::default(),
            jobs: Vec::new(),
            agent: AgentView::default(),
            settings: state.settings.clone(),
            auto_start: state.auto_start,
            console: Vec::new(),
            console_scroll: 0,
            help_scroll: 0,
            notices: Vec::new(),
            history: state.history.iter().take(MAX_HISTORY).cloned().collect(),
            input: None,
            form: None,
            confirm: None,
            delete_armed: None,
            restart_pending: false,
            children: Vec::new(),
            last_session_end: state.last_session_end,
            session_started: Utc::now(),
            quit: false,
            dirty: false,
            last_save: Instant::now(),
            save_error: None,
        };

        if let Some(cache) = state.cache {
            if cache.status.is_some() {
                app.status = cache.status;
                app.peers = cache.peers;
                app.shares = cache.shares;
                app.extensions = cache.extensions;
                app.cached_at = Some(cache.at);
            }
        }
        app.live = agent::socket_ready(&socket);

        for job in state.jobs {
            if agent::pid_alive(job.pid) {
                app.jobs.push(Job {
                    pid: job.pid,
                    label: job.label,
                    log: job.log,
                    stopping: false,
                });
            }
        }
        app.jobs_table
            .select((!app.jobs.is_empty()).then_some(0));

        app.notices = state
            .notices
            .into_iter()
            .rev()
            .take(MAX_NOTICES)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|notice| Notice {
                at: notice.at,
                level: Level::from_persisted(&notice.level),
                text: persist::sanitize(&notice.text),
            })
            .collect();

        if let Some(end) = app.last_session_end {
            app.push_console_previous(
                Level::Info,
                format!(
                    "── sessão anterior encerrada em {} ──",
                    end.with_timezone(&chrono::Local).format("%d/%m %H:%M:%S")
                ),
            );
        } else {
            app.push_console_previous(Level::Info, "── início da sessão ──");
        }
        for event in persist::read_events(&app.root, 80) {
            app.console.push(ConsoleLine {
                level: Level::from_persisted(&event.level),
                text: format!(
                    "{} {}",
                    event
                        .ts
                        .with_timezone(&chrono::Local)
                        .format("%H:%M:%S"),
                    event.text
                ),
                previous: true,
            });
        }
        app.trim_console();

        if let Some(warning) = warning {
            app.notify(Level::Warn, warning);
        }
        match persist::load_enrolled(&app.root) {
            Ok(node) => app.node = node,
            Err(err) => {
                app.node_error = Some(err.clone());
                app.notify(Level::Warn, err);
            }
        }
        if app.peers.is_empty() {
            app.table.select(None);
        } else {
            app.table
                .select(Some(peer_index.min(app.peers.len() - 1)));
        }
        if app.shares.is_empty() {
            app.shares_table.select(None);
        } else {
            app.shares_table
                .select(Some(share_index.min(app.shares.len() - 1)));
        }
        app
    }

    pub fn is_enrolled(&self) -> bool {
        self.node.is_some()
    }

    pub fn is_own_share(&self, share: &ShareInfo) -> bool {
        self.status
            .as_ref()
            .and_then(|status| status.node_id)
            .map(|id| id == share.node_id)
            .unwrap_or(false)
    }

    pub fn selected_peer(&self) -> Option<&PeerStatus> {
        self.table.selected().and_then(|index| self.peers.get(index))
    }

    pub fn selected_share(&self) -> Option<&ShareInfo> {
        self.shares_table
            .selected()
            .and_then(|index| self.shares.get(index))
    }

    pub fn selected_job(&self) -> Option<&Job> {
        self.jobs_table
            .selected()
            .and_then(|index| self.jobs.get(index))
    }

    pub fn agent_label(&self) -> String {
        if self.agent.stopping.is_some() {
            return "parando…".into();
        }
        if self.agent.starting.is_some() {
            let seconds = self
                .agent
                .starting
                .map(|started| started.elapsed().as_secs())
                .unwrap_or(0);
            return format!("iniciando há {seconds}s");
        }
        if let Some(pid) = self.agent.pid {
            if self.live {
                return format!("ativo ({pid})");
            }
        }
        if self.live {
            "ativo".into()
        } else {
            "parado".into()
        }
    }

    pub fn notify(&mut self, level: Level, text: impl Into<String>) {
        let text = persist::sanitize(&text.into());
        let at = Utc::now();
        let headline = text.split('\n').next().unwrap_or_default().to_string();
        self.notices.push(Notice {
            at,
            level,
            text: headline,
        });
        if self.notices.len() > MAX_NOTICES {
            let excess = self.notices.len() - MAX_NOTICES;
            self.notices.drain(..excess);
        }
        self.push_console(level, text.clone());
        persist::append_event(&self.root, level.persist_name(), &text);
        self.dirty = true;
    }

    pub fn last_notice(&self) -> Option<&Notice> {
        self.notices.last()
    }

    pub fn push_console(&mut self, level: Level, text: impl Into<String>) {
        let text = text.into();
        let before = self.console.len();
        for line in text.split('\n') {
            let line = persist::sanitize(line);
            let line = if line.len() > 4_000 {
                let mut cut = 4_000;
                while !line.is_char_boundary(cut) {
                    cut -= 1;
                }
                format!("{}…", &line[..cut])
            } else {
                line.to_string()
            };
            self.console.push(ConsoleLine {
                level,
                text: line,
                previous: false,
            });
        }
        let added = self.console.len().saturating_sub(before);
        self.trim_console();
        if self.console_scroll > 0 {
            self.console_scroll = self.console_scroll.saturating_add(added);
        }
        self.clamp_console_scroll();
    }

    fn push_console_previous(&mut self, level: Level, text: impl Into<String>) {
        self.console.push(ConsoleLine {
            level,
            text: text.into(),
            previous: true,
        });
    }

    fn trim_console(&mut self) {
        if self.console.len() > MAX_CONSOLE_LINES {
            let excess = self.console.len() - MAX_CONSOLE_LINES;
            self.console.drain(..excess);
            self.console_scroll = self.console_scroll.saturating_sub(excess);
        }
    }

    pub fn clamp_console_scroll(&mut self) {
        let max = self.console.len().saturating_sub(1);
        if self.console_scroll > max {
            self.console_scroll = max;
        }
    }

    pub fn scroll_console(&mut self, delta: isize) {
        let max = self.console.len().saturating_sub(1);
        let next = (self.console_scroll as isize + delta).clamp(0, max as isize);
        self.console_scroll = next as usize;
    }

    pub fn push_history(&mut self, line: &str) {
        let line = persist::redact_input(line);
        if line.trim().is_empty() {
            return;
        }
        if self.history.last().is_some_and(|last| *last == line) {
            return;
        }
        self.history.push(line);
        if self.history.len() > MAX_HISTORY {
            let excess = self.history.len() - MAX_HISTORY;
            self.history.drain(..excess);
        }
        self.dirty = true;
    }

    pub fn apply_snapshot(&mut self, snapshot: crate::Snapshot) {
        self.error = snapshot.error;
        if let Some(status) = snapshot.status {
            if self.node.is_none() {
                if let (Some(name), Some(ip)) = (status.node_name.clone(), status.overlay_ip) {
                    self.node = Some(EnrolledNode {
                        node_name: name,
                        overlay_ip: ip.to_string(),
                        control_url: status.control_url.clone().unwrap_or_default(),
                        network_name: String::new(),
                        created_at: None,
                    });
                }
            }
            if let Some(node) = self.node.as_mut() {
                if let Some(name) = status.node_name.clone() {
                    node.node_name = name;
                }
                if let Some(ip) = status.overlay_ip {
                    node.overlay_ip = ip.to_string();
                }
                if let Some(control_url) = status.control_url.clone() {
                    node.control_url = control_url;
                }
            }
            self.node_error = None;
            self.status = Some(status);
            self.peers = snapshot.peers;
            self.shares = snapshot.shares;
            self.extensions = snapshot.extensions;
            self.cached_at = Some(Utc::now());
            self.live = true;
            if self.agent.starting.is_some() {
                self.agent.starting = None;
            }
        } else {
            self.live = false;
        }
        if !snapshot.log.is_empty() {
            self.agent.log = snapshot
                .log
                .into_iter()
                .map(|line| persist::sanitize(&line))
                .collect();
        }
        if snapshot.pid.is_some() {
            self.agent.pid = snapshot.pid;
        }
        self.updated = Some(Instant::now());
        self.clamp_tables();
    }

    fn clamp_tables(&mut self) {
        if self
            .table
            .selected()
            .is_none_or(|index| index >= self.peers.len())
        {
            self.table
                .select((!self.peers.is_empty()).then_some(0));
        }
        if self
            .shares_table
            .selected()
            .is_none_or(|index| index >= self.shares.len())
        {
            self.shares_table
                .select((!self.shares.is_empty()).then_some(0));
        }
        if self
            .jobs_table
            .selected()
            .is_none_or(|index| index >= self.jobs.len())
        {
            self.jobs_table
                .select((!self.jobs.is_empty()).then_some(0));
        }
    }

    pub fn add_job(&mut self, pid: u32, label: String, log: PathBuf) {
        if let Some(job) = self.jobs.iter_mut().find(|job| job.pid == pid) {
            job.label = label;
            job.log = log;
            job.stopping = false;
        } else {
            self.jobs.push(Job {
                pid,
                label,
                log,
                stopping: false,
            });
        }
        if self.jobs_table.selected().is_none() {
            self.jobs_table.select(Some(self.jobs.len() - 1));
        }
        self.dirty = true;
    }

    pub fn remove_job(&mut self, pid: u32) -> Option<Job> {
        let index = self.jobs.iter().position(|job| job.pid == pid)?;
        let job = self.jobs.remove(index);
        self.clamp_tables();
        self.dirty = true;
        Some(job)
    }

    pub fn cache(&self) -> Option<SnapshotCache> {
        let status = self.status.clone()?;
        Some(SnapshotCache {
            at: self.cached_at.unwrap_or_else(Utc::now),
            status: Some(status),
            peers: self.peers.clone(),
            shares: self.shares.clone(),
            extensions: self.extensions.clone(),
        })
    }

    pub fn to_persisted(&self) -> PersistedState {
        PersistedState {
            version: persist::STATE_VERSION,
            tab: self.tab,
            peer_index: self.table.selected().unwrap_or(0),
            share_index: self.shares_table.selected().unwrap_or(0),
            auto_start: self.auto_start,
            settings: self.settings.clone(),
            history: self.history.clone(),
            notices: self
                .notices
                .iter()
                .rev()
                .take(MAX_NOTICES)
                .rev()
                .map(|notice| NoticeRecord {
                    at: notice.at,
                    level: notice.level.persist_name().to_string(),
                    text: persist::redact(&notice.text),
                })
                .collect(),
            jobs: self
                .jobs
                .iter()
                .filter(|job| agent::pid_alive(job.pid))
                .map(|job| PersistedJob {
                    pid: job.pid,
                    label: job.label.clone(),
                    log: job.log.clone(),
                    started_at: None,
                })
                .collect(),
            cache: self.cache(),
            last_session_start: Some(self.session_started),
            last_session_end: self.last_session_end,
        }
    }

    pub fn save(&mut self) -> bool {
        match persist::save_state(&self.root, &self.to_persisted()) {
            Ok(()) => {
                self.dirty = false;
                self.save_error = None;
                true
            }
            Err(err) => {
                let message = format!("não foi possível salvar o estado da TUI: {err}");
                let first_failure = self.save_error.as_deref() != Some(message.as_str());
                if first_failure {
                    self.notify(Level::Warn, message.clone());
                    self.save_error = Some(message);
                }
                // A failed periodic write should not retry every second (and
                // should never spin the event loop).
                self.dirty = false;
                false
            }
        }
    }
}

pub fn join_form() -> Form {
    Form::new(
        "Entrar na malha",
        FormAction::Join,
        vec![
            Field::text(
                "código wmc_",
                "",
                512,
                "cole o convite wmc_ ou wm://join (vence URL+token)",
            ),
            Field::text(
                "URL do control",
                "",
                300,
                "ex.: https://mesh.exemplo.com; use com o token",
            ),
            Field::text("token wmj_", "", 400, "convite manual; não é gravado em disco"),
            Field::text("nome deste nó", "", 63, "opcional; padrão é o hostname"),
        ],
    )
    .with_note("a inscrição é gravada em agent-state.json; o agente inicia em seguida")
}

pub fn new_share_form() -> Form {
    Form::new(
        "Publicar serviço",
        FormAction::NewShare,
        vec![
            Field::digits("porta local", "3000", "porta TCP/UDP do serviço nesta máquina"),
            Field::text("nome (wm://)", "", 63, "slug único na malha; vazio usa port-<porta>"),
            Field::choice(
                "tipo",
                "web",
                &["web", "tcp", "udp", "game"],
                "só apresenta o serviço; game assume TCP+UDP",
            ),
            Field::choice(
                "transporte",
                "auto",
                &["auto", "tcp", "udp", "both"],
                "auto: web=tcp, udp=udp, game=tcp+udp",
            ),
            Field::text("descrição", "", 240, "texto curto mostrado nos painéis"),
            Field::boolean("ativo", true, "desligado mantém o share no inventário"),
        ],
    )
    .with_note("o serviço precisa escutar no overlay/0.0.0.0; use --expose para loopback")
}

pub fn rename_share_form(share: &ShareInfo) -> Form {
    Form::new(
        "Renomear share",
        FormAction::RenameShare(share.id),
        vec![Field::text(
            "novo nome",
            &share.name,
            63,
            "slug único na malha",
        )],
    )
}

pub fn rename_node_form(current: &str) -> Form {
    Form::new(
        "Renomear este nó",
        FormAction::RenameNode,
        vec![Field::text("novo nome", current, 63, "nome usado no mapa e nos painéis")],
    )
}

pub fn agent_settings_form(settings: &AgentSettings) -> Form {
    Form::new(
        "Configurações do agente",
        FormAction::AgentSettings,
        vec![
            Field::digits(
                "porta UDP",
                &settings.udp_port.to_string(),
                "porta do data plane WireGuard",
            ),
            Field::digits("MTU", &settings.mtu.to_string(), "normalmente 1280"),
            Field::boolean(
                "forçar relay",
                settings.force_relay,
                "ignora caminho direto; útil em rede com UDP bloqueado",
            ),
            Field::text(
                "expor portas",
                &settings.expose,
                200,
                "lista vírgula: 8421 ou 9000:9001 (overlay:local)",
            ),
            Field::text(
                "nome do TUN",
                &settings.tun_name,
                32,
                "vazio deixa o sistema escolher",
            ),
        ],
    )
    .with_note("aplicado na próxima inicialização do agente")
}

pub fn probe_form() -> Form {
    Form::new(
        "Testar porta",
        FormAction::ProbePort,
        vec![Field::digits("porta TCP", "8421", "porta testada no peer selecionado")],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use wm_proto::{NatType, PathKind, ShareKind, ShareProtocol};

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "wm-tui-app-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    pub fn sample_status() -> AgentStatus {
        AgentStatus {
            version: "0.1.0".into(),
            node_id: Some(Uuid::new_v4()),
            node_name: Some("site".into()),
            overlay_ip: Some("10.66.0.1".parse().unwrap()),
            control_url: Some("https://mesh.example.com".into()),
            control_connected: true,
            relay_connected: true,
            tun_name: Some("tun0".into()),
            udp_port: Some(41641),
            nat_type: NatType::Cone,
            peers_total: 2,
            peers_online: 2,
            uptime_secs: 42,
            map_version: 3,
            last_error: None,
        }
    }

    pub fn sample_peer(name: &str, path: PathKind) -> PeerStatus {
        PeerStatus {
            id: Uuid::new_v4(),
            name: name.into(),
            overlay_ip: "10.66.0.12".parse().unwrap(),
            path,
            endpoint: Some("203.0.113.9:41641".into()),
            rtt_ms: Some(4),
            last_handshake_secs: Some(2),
            rx_bytes: 1024,
            tx_bytes: 2048,
            relay_online: true,
            last_error: None,
        }
    }

    pub fn sample_share() -> ShareInfo {
        ShareInfo {
            id: Uuid::new_v4(),
            name: "demo".into(),
            node_id: Uuid::new_v4(),
            node_name: "mac-01".into(),
            overlay_ip: "10.66.0.12".parse().unwrap(),
            port: 3000,
            kind: ShareKind::Web,
            protocol: ShareProtocol::Tcp,
            description: "preview".into(),
            enabled: true,
            online: true,
            public_url: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn notify_keeps_console_notices_and_event_log() {
        let root = temp_root("notify");
        let mut app = App::load(root.clone());
        app.notify(Level::Good, "agente ativo");
        assert_eq!(app.notices.len(), 1);
        assert!(app.console.iter().any(|line| line.text == "agente ativo"));
        let events = persist::read_events(&root, 5);
        assert!(events.iter().any(|event| event.text == "agente ativo"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn snapshot_populates_and_keeps_stale_data_when_offline() {
        let root = temp_root("snapshot");
        let mut app = App::load(root.clone());
        let share = sample_share();
        app.apply_snapshot(crate::Snapshot {
            status: Some(sample_status()),
            peers: vec![sample_peer("mac-01", PathKind::Direct)],
            shares: vec![share.clone()],
            extensions: Vec::new(),
            error: None,
            log: vec!["linha".into()],
            pid: Some(123),
        });
        assert!(app.live);
        assert_eq!(app.peers.len(), 1);
        assert_eq!(app.selected_share().map(|s| s.name.as_str()), Some("demo"));
        app.apply_snapshot(crate::Snapshot {
            status: None,
            peers: Vec::new(),
            shares: Vec::new(),
            extensions: Vec::new(),
            error: Some("socket fora".into()),
            log: Vec::new(),
            pid: None,
        });
        assert!(!app.live);
        assert_eq!(app.peers.len(), 1, "dados antigos continuam visíveis");
        assert_eq!(app.shares.len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persisted_roundtrip_restores_tab_history_settings_and_cache() {
        let root = temp_root("restore");
        let mut app = App::load(root.clone());
        app.tab = TAB_AGENT;
        app.auto_start = false;
        app.settings.udp_port = 9999;
        app.push_history("status");
        app.push_history("join https://mesh.test --token wmj_abcdefghijklmnop");
        app.apply_snapshot(crate::Snapshot {
            status: Some(sample_status()),
            peers: vec![
                sample_peer("mac-01", PathKind::Direct),
                sample_peer("mac-02", PathKind::Relay),
            ],
            shares: vec![sample_share()],
            extensions: Vec::new(),
            error: None,
            log: Vec::new(),
            pid: Some(77),
        });
        app.table.select(Some(1));
        app.notify(Level::Info, "teste");
        assert!(app.save());

        let restored = App::load(root.clone());
        assert_eq!(restored.tab, TAB_AGENT);
        assert!(!restored.auto_start);
        assert_eq!(restored.settings.udp_port, 9999);
        assert_eq!(restored.history.len(), 2);
        assert!(restored
            .history
            .iter()
            .all(|line| !line.contains("abcdefghijklmnop")));
        assert_eq!(restored.cached_at.is_some(), true);
        assert_eq!(restored.shares.len(), 1);
        assert_eq!(restored.peers.len(), 2);
        assert_eq!(restored.table.selected(), Some(1));
        assert!(restored.console.iter().any(|line| line.previous));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn live_jobs_persist_and_dead_pids_are_dropped() {
        let root = temp_root("jobs");
        let mut app = App::load(root.clone());
        app.add_job(
            std::process::id(),
            "gui".into(),
            root.join("gui.log"),
        );
        app.add_job(u32::MAX, "host".into(), root.join("host.log"));
        assert_eq!(app.jobs.len(), 2);
        assert!(app.save());
        let restored = App::load(root.clone());
        assert_eq!(restored.jobs.len(), 1);
        assert_eq!(restored.jobs[0].label, "gui");
        assert_eq!(restored.jobs[0].pid, std::process::id());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn console_is_bounded_and_scroll_is_clamped() {
        let root = temp_root("console");
        let mut app = App::load(root.clone());
        for index in 0..(MAX_CONSOLE_LINES + 50) {
            app.push_console(Level::Output, format!("linha {index}"));
        }
        assert_eq!(app.console.len(), MAX_CONSOLE_LINES);
        app.scroll_console(10_000);
        assert!(app.console_scroll <= app.console.len() - 1);
        app.scroll_console(-100_000);
        assert_eq!(app.console_scroll, 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn field_choice_cycles_within_options() {
        let mut field = Field::choice("tipo", "web", &["web", "tcp", "udp"], "");
        field.cycle(true);
        assert_eq!(field.value, "tcp");
        field.cycle(true);
        assert_eq!(field.value, "udp");
        field.cycle(true);
        assert_eq!(field.value, "web", "choices wrap around");
        field.cycle(false);
        assert_eq!(field.value, "udp");
        let mut flag = Field::boolean("ativo", true, "");
        assert!(flag.truthy());
        flag.cycle(false);
        assert!(!flag.truthy());
    }
}
