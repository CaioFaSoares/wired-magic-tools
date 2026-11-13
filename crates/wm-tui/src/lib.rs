//! wired-magic terminal UI (Ratatui).
//!
//! The TUI is a full local operator console: it can enroll this node, start a
//! detached agent that survives closing the terminal, manage shares, run any
//! `wm` subcommand and restore its own session on the next start. It still
//! talks to the agent only through the Unix-socket local API, so it never
//! touches WireGuard or the network directly.

mod actions;
mod agent;
mod app;
mod command;
mod persist;
mod ui;

use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use chrono::Utc;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::cursor::Show;
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use wm_proto::{AgentStatus, ExtensionStatus, PeerStatus, ShareInfo};

use crate::agent::{AgentEvent, JobEvent};
use crate::app::{
    App, FieldKind, Level, TABS, TAB_AGENT, TAB_CONSOLE, TAB_HELP, TAB_PEERS, TAB_SERVICES,
    TAB_SHARES, TAB_STATUS,
};
use crate::persist::EnrolledNode;

/// Options for one TUI session.
#[derive(Debug, Clone, Copy, Default)]
pub struct RunOptions {
    /// Do not start the agent automatically when this node is already enrolled.
    pub no_auto_start: bool,
}

/// One poll of the local agent, including the agent's own log tail.
pub struct Snapshot {
    pub status: Option<AgentStatus>,
    pub peers: Vec<PeerStatus>,
    pub shares: Vec<ShareInfo>,
    pub extensions: Vec<ExtensionStatus>,
    pub error: Option<String>,
    pub log: Vec<String>,
    pub pid: Option<u32>,
}

pub enum Msg {
    Update(Snapshot),
    Notify(Level, String),
    Console(Level, String),
    Agent(AgentEvent),
    Job(JobEvent),
    Joined(Option<EnrolledNode>),
    CommandFinished {
        pid: u32,
        label: String,
        code: Option<i32>,
        success: bool,
    },
}

struct Screen {
    active: bool,
}

impl Screen {
    fn enter() -> anyhow::Result<Self> {
        enable_raw_mode().map_err(|err| {
            anyhow::anyhow!("a TUI precisa de um terminal interativo: {err}")
        })?;
        if let Err(err) = execute!(std::io::stdout(), EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(err.into());
        }
        Ok(Self { active: true })
    }

    fn restore(&mut self) {
        if !self.active {
            return;
        }
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen, Show);
        self.active = false;
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Runs the TUI with the default options.
pub fn run(data_dir: PathBuf) -> anyhow::Result<()> {
    run_with(data_dir, RunOptions::default())
}

/// Runs the TUI until `q`. The agent process, however, is deliberately left
/// running (detached) and reconnects on the next session.
pub fn run_with(data_dir: PathBuf, options: RunOptions) -> anyhow::Result<()> {
    // Detached children inherit the working directory, but an absolute root
    // keeps the TUI and the agent on the same data even if a shell changes it.
    let data_dir = if data_dir.is_absolute() {
        data_dir
    } else {
        std::env::current_dir()?.join(data_dir)
    };
    persist::ensure_dir(&data_dir)?;
    let mut app = App::load(data_dir);
    let (tx, rx) = mpsc::channel();
    start_fetcher(
        app.root.clone(),
        app.socket.clone(),
        app.log_path.clone(),
        tx.clone(),
    );

    let mut screen = Screen::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
    terminal.hide_cursor()?;

    app.notify(
        Level::Info,
        "TUI pronta — q sai e mantém o agente ativo em segundo plano",
    );
    maybe_auto_start(&mut app, &tx, options.no_auto_start);
    app.save();
    app.last_save = Instant::now();

    let result = event_loop(&mut terminal, &mut app, rx, tx.clone());

    for pid in app.children.clone() {
        let _ = agent::signal(pid, libc::SIGTERM);
    }
    app.last_session_end = Some(Utc::now());
    let saved = app.save();
    screen.restore();
    if result.is_ok() {
        print_summary(&app, saved);
    }
    result
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
    rx: Receiver<Msg>,
    tx: Sender<Msg>,
) -> anyhow::Result<()> {
    loop {
        while let Ok(msg) = rx.try_recv() {
            handle_msg(app, msg, &tx);
        }
        if app.quit {
            break;
        }
        terminal.draw(|frame| ui::draw(frame, app))?;
        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                handle_key(app, key, &tx);
            }
        }
        let interval = if app.dirty {
            Duration::from_secs(1)
        } else {
            Duration::from_secs(10)
        };
        if app.last_save.elapsed() >= interval {
            app.save();
            app.last_save = Instant::now();
        }
    }
    Ok(())
}

fn maybe_auto_start(app: &mut App, tx: &Sender<Msg>, no_auto_start: bool) {
    if !app.is_enrolled() {
        return;
    }
    if no_auto_start {
        app.notify(
            Level::Info,
            "auto-start desativado nesta sessão (--no-auto-start)",
        );
        return;
    }
    if agent::socket_ready(&app.socket) {
        app.live = true;
        app.notify(Level::Info, "agente já estava ativo — sessão retomada");
        return;
    }
    if !app.auto_start {
        app.notify(
            Level::Info,
            "agente parado; auto-start desligado (pressione s para iniciar)",
        );
        return;
    }
    if !actions::is_root() {
        app.notify(
            Level::Warn,
            "agente parado; sem root a TUI não pode iniciá-lo (use sudo wm tui)",
        );
        return;
    }
    app.notify(Level::Info, "auto-start: iniciando o agente em segundo plano…");
    actions::start_agent(app, tx);
}

fn start_fetcher(root: PathBuf, socket: PathBuf, log: PathBuf, tx: Sender<Msg>) {
    std::thread::spawn(move || {
        loop {
            let (status, error) =
                match actions::local_call(&socket, "status", Value::Null) {
                    Ok(value) => match serde_json::from_value::<AgentStatus>(value) {
                        Ok(status) => (Some(status), None),
                        Err(err) => (None, Some(format!("status inválido: {err}"))),
                    },
                    Err(err) => (None, Some(err)),
                };
            let (peers, shares, extensions) = if status.is_some() {
                (
                    fetch::<PeerStatus>(&socket, "peers"),
                    fetch::<ShareInfo>(&socket, "shares"),
                    fetch::<ExtensionStatus>(&socket, "extensions"),
                )
            } else {
                (Vec::new(), Vec::new(), Vec::new())
            };
            let pid = agent::read_pid(&root);
            let log_lines = agent::log_tail(&log);
            if tx
                .send(Msg::Update(Snapshot {
                    status,
                    peers,
                    shares,
                    extensions,
                    error,
                    log: log_lines,
                    pid,
                }))
                .is_err()
            {
                return;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
}

fn fetch<T: DeserializeOwned>(socket: &PathBuf, method: &str) -> Vec<T> {
    actions::local_call(socket, method, Value::Null)
        .and_then(|value| serde_json::from_value(value).map_err(|err| err.to_string()))
        .unwrap_or_default()
}

fn handle_msg(app: &mut App, msg: Msg, tx: &Sender<Msg>) {
    match msg {
        Msg::Update(snapshot) => {
            let was_live = app.live;
            let had_snapshot = app.updated.is_some();
            let managed = app.agent.managed;
            app.apply_snapshot(snapshot);
            if !was_live && app.live && had_snapshot && !managed {
                app.notify(Level::Good, "agente conectado");
            }
            if was_live
                && !app.live
                && app.agent.stopping.is_none()
                && app.agent.starting.is_none()
                && !managed
            {
                app.notify(
                    Level::Warn,
                    "o agente parou; veja o log na aba Agente",
                );
            }
        }
        Msg::Notify(level, text) => app.notify(level, text),
        Msg::Console(level, text) => app.push_console(level, text),
        Msg::Agent(event) => handle_agent_event(app, event, tx),
        Msg::Job(event) => handle_job_event(app, event),
        Msg::Joined(node) => {
            let name = node
                .as_ref()
                .map(|node| format!("{} ({})", node.node_name, node.overlay_ip));
            app.node = node;
            app.node_error = None;
            app.auto_start = true;
            app.dirty = true;
            match name {
                Some(name) => app.notify(Level::Good, format!("nó inscrito: {name}")),
                None => app.notify(
                    Level::Warn,
                    "a inscrição terminou, mas agent-state.json não foi encontrado",
                ),
            }
        }
        Msg::CommandFinished {
            pid,
            label,
            code,
            success,
        } => {
            app.children.retain(|child| *child != pid);
            let detail = code
                .map(|code| format!("código {code}"))
                .unwrap_or_else(|| "terminou".into());
            if success {
                app.notify(Level::Good, format!("{label}: concluído"));
            } else {
                app.notify(Level::Warn, format!("{label}: {detail}"));
            }
            if label == "join" {
                match persist::load_enrolled(&app.root) {
                    Ok(node) => {
                        if node.is_some() {
                            app.node = node;
                            app.dirty = true;
                        }
                    }
                    Err(err) => app.notify(Level::Warn, err),
                }
            }
        }
    }
}

fn finish_stop(app: &mut App, tx: &Sender<Msg>) {
    app.agent.starting = None;
    app.agent.stopping = None;
    app.agent.pid = None;
    app.agent.managed = false;
    app.live = false;
    if app.restart_pending {
        app.restart_pending = false;
        app.notify(Level::Info, "agente parado; reiniciando…");
        actions::start_agent(app, tx);
    } else {
        app.auto_start = false;
        app.notify(Level::Good, "agente parado; a inscrição continua salva");
    }
}

fn handle_agent_event(app: &mut App, event: AgentEvent, tx: &Sender<Msg>) {
    match event {
        AgentEvent::Starting { pid } => {
            app.agent.starting = Some(Instant::now());
            app.agent.stopping = None;
            app.agent.pid = Some(pid);
            app.agent.managed = true;
        }
        AgentEvent::Started { pid } => {
            app.agent.starting = None;
            app.agent.stopping = None;
            app.agent.pid = Some(pid);
            app.agent.managed = true;
            app.agent.last_error = None;
            app.agent.last_stop_pid = None;
            app.live = true;
            app.auto_start = true;
            app.notify(
                Level::Good,
                format!("agente ativo (pid {pid}) — continua após sair com q"),
            );
        }
        AgentEvent::StartFailed { pid, code, tail } => {
            app.agent.starting = None;
            app.agent.stopping = None;
            if app.agent.pid == Some(pid) {
                app.agent.pid = None;
            }
            let reason = code
                .map(|code| format!("o processo terminou com código {code}"))
                .unwrap_or_else(|| "o processo não iniciou".into());
            app.agent.last_error = Some(reason.clone());
            for line in tail.iter().rev().take(6).rev() {
                app.push_console(Level::Warn, line.clone());
            }
            app.notify(
                Level::Error,
                format!(
                    "agente não iniciou (pid {pid}): {reason}. Veja {}",
                    app.log_path.display()
                ),
            );
        }
        AgentEvent::Exited { pid, code } => {
            if app.agent.last_stop_pid == Some(pid) {
                app.agent.last_stop_pid = None;
                return;
            }
            let intentional = app.agent.stopping.is_some() || app.restart_pending;
            app.agent.starting = None;
            app.agent.stopping = None;
            if app.agent.pid == Some(pid) {
                app.agent.pid = None;
            }
            app.live = false;
            if intentional {
                app.agent.last_stop_pid = Some(pid);
                finish_stop(app, tx);
            } else {
                let code = code
                    .map(|code| format!(" (código {code})"))
                    .unwrap_or_default();
                app.notify(
                    Level::Warn,
                    format!("agente encerrou{code}; veja o log na aba Agente"),
                );
            }
        }
        AgentEvent::Stopped { pid } => {
            if app.agent.last_stop_pid == Some(pid) {
                app.agent.last_stop_pid = None;
                return;
            }
            app.agent.last_stop_pid = Some(pid);
            app.agent.stopping = None;
            app.agent.starting = None;
            app.agent.pid = None;
            app.live = false;
            finish_stop(app, tx);
        }
        AgentEvent::StopTimeout { pid } => {
            app.agent.stopping = None;
            app.restart_pending = false;
            app.auto_start = false;
            app.notify(
                Level::Error,
                format!("o agente (pid {pid}) não parou em 8s; verifique com `wm down`"),
            );
        }
        AgentEvent::StopFailed { pid, error } => {
            app.agent.stopping = None;
            app.restart_pending = false;
            app.notify(
                Level::Error,
                format!("não foi possível parar o agente (pid {pid}): {error}"),
            );
        }
        AgentEvent::NoAgent => {
            app.agent.stopping = None;
            app.restart_pending = false;
            app.auto_start = false;
            app.notify(Level::Info, "nenhum agente em execução; auto-start desligado");
        }
    }
}

fn handle_job_event(app: &mut App, event: JobEvent) {
    match event {
        JobEvent::Started { pid, label, log } => {
            app.add_job(pid, label, log);
        }
        JobEvent::Exited { pid, label, code } => {
            app.remove_job(pid);
            match code {
                Some(0) | None => app.notify(Level::Info, format!("{label} terminou")),
                Some(code) => app.notify(
                    Level::Warn,
                    format!("{label} terminou com código {code}; veja o log na aba Serviços"),
                ),
            }
        }
        JobEvent::StopTimeout { pid, label } => {
            if let Some(job) = app.jobs.iter_mut().find(|job| job.pid == pid) {
                job.stopping = false;
            }
            app.notify(
                Level::Error,
                format!("{label} (pid {pid}) não parou em 8s"),
            );
        }
    }
}

fn handle_key(app: &mut App, key: KeyEvent, tx: &Sender<Msg>) {
    if key.kind != KeyEventKind::Press {
        return;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        app.quit = true;
        return;
    }
    if app.form.is_some() {
        handle_form_key(app, key, tx);
        return;
    }
    if app.confirm.is_some() {
        handle_confirm_key(app, key, tx);
        return;
    }
    if app.input.is_some() {
        handle_input_key(app, key, tx);
        return;
    }
    match key.code {
        KeyCode::Char('q') => {
            app.quit = true;
            return;
        }
        KeyCode::Char('?') => {
            app.tab = TAB_HELP;
            return;
        }
        KeyCode::Char(':') => {
            actions::open_console(app);
            return;
        }
        KeyCode::Char(digit @ '1'..='8') => {
            app.tab = (digit as usize - '1' as usize).min(TABS.len() - 1);
            return;
        }
        KeyCode::Tab | KeyCode::Right => {
            app.tab = (app.tab + 1) % TABS.len();
            app.delete_armed = None;
            return;
        }
        KeyCode::BackTab | KeyCode::Left => {
            app.tab = (app.tab + TABS.len() - 1) % TABS.len();
            app.delete_armed = None;
            return;
        }
        _ => {}
    }

    if !app.is_enrolled()
        && app.tab == TAB_STATUS
        && key.code == KeyCode::Char('j')
    {
        app.form = Some(app::join_form());
        return;
    }

    match key.code {
        KeyCode::Down | KeyCode::Char('j') => navigate(app, true),
        KeyCode::Up | KeyCode::Char('k') => navigate(app, false),
        KeyCode::PageDown => {
            if app.tab == TAB_CONSOLE {
                app.scroll_console(-10);
            } else {
                navigate_by(app, 10);
            }
        }
        KeyCode::PageUp => {
            if app.tab == TAB_CONSOLE {
                app.scroll_console(10);
            } else {
                navigate_by(app, -10);
            }
        }
        KeyCode::Home => match app.tab {
            TAB_CONSOLE => app.console_scroll = usize::MAX / 2,
            TAB_HELP => app.help_scroll = 0,
            _ => {}
        },
        KeyCode::End => match app.tab {
            TAB_CONSOLE => app.console_scroll = 0,
            TAB_HELP => app.help_scroll = usize::MAX / 2,
            _ => {}
        },
        KeyCode::Enter => match app.tab {
            TAB_SHARES => actions::open_share(app),
            TAB_PEERS => actions::open_probe_form(app),
            TAB_SERVICES => actions::view_job_log(app),
            _ => {}
        },
        KeyCode::Char('c') if app.tab == TAB_CONSOLE => {
            app.console.clear();
            app.console_scroll = 0;
        }
        _ => handle_tab_action(app, key, tx),
    }
}

fn handle_tab_action(app: &mut App, key: KeyEvent, tx: &Sender<Msg>) {
    match app.tab {
        TAB_STATUS | TAB_AGENT => match key.code {
            KeyCode::Char('s') => actions::start_agent(app, tx),
            KeyCode::Char('X') => actions::confirm_stop_agent(app),
            KeyCode::Char('R') => actions::confirm_restart_agent(app),
            KeyCode::Char('e') => {
                app.form = Some(app::agent_settings_form(&app.settings));
            }
            KeyCode::Char('a') => toggle_auto_start(app, tx),
            KeyCode::Char('g') => actions::open_gui(app, tx),
            KeyCode::Char('d') => actions::run_doctor(app, tx),
            KeyCode::Char('l') => actions::view_agent_log(app),
            KeyCode::Char('i') if app.tab == TAB_AGENT => {
                actions::confirm_install_service(app)
            }
            KeyCode::Char('n') if app.tab == TAB_STATUS => open_rename_node(app),
            _ => {}
        },
        TAB_PEERS => match key.code {
            KeyCode::Char('p') => actions::ping_peer(app, tx),
            KeyCode::Char('o') => actions::open_probe_form(app),
            KeyCode::Char('x') => actions::reconnect_peer(app, tx),
            KeyCode::Char('c') => actions::copy_selected_peer(app),
            KeyCode::Char('n') => open_rename_node(app),
            _ => {}
        },
        TAB_SHARES => match key.code {
            KeyCode::Char('n') => open_new_share(app),
            KeyCode::Char('r') => {
                if let Some(share) = app.selected_share().cloned() {
                    if app.is_own_share(&share) {
                        app.form = Some(app::rename_share_form(&share));
                    } else {
                        app.notify(Level::Warn, "só o dono pode renomear este share");
                    }
                }
            }
            KeyCode::Char('e') => actions::toggle_share(app, tx),
            KeyCode::Char('d') => actions::request_delete_share(app, tx),
            KeyCode::Char('c') => actions::copy_selected_share(app),
            _ => {}
        },
        TAB_SERVICES => match key.code {
            KeyCode::Char('g') => actions::open_gui(app, tx),
            KeyCode::Char('G') => actions::confirm_stop_gui(app),
            KeyCode::Char('x') => actions::confirm_stop_selected_job(app),
            KeyCode::Char('i') => actions::confirm_install_service(app),
            KeyCode::Char('u') => actions::confirm_uninstall_service(app),
            KeyCode::Char('t') => actions::service_status(app, tx),
            KeyCode::Char('d') => actions::run_doctor(app, tx),
            KeyCode::Char('p') => actions::open_admin_panel(app),
            _ => {}
        },
        _ => {}
    }
}

fn toggle_auto_start(app: &mut App, tx: &Sender<Msg>) {
    if !app.is_enrolled() {
        app.notify(Level::Warn, "entre na malha antes de configurar o auto-start");
        return;
    }
    app.auto_start = !app.auto_start;
    app.dirty = true;
    if app.auto_start {
        app.notify(
            Level::Info,
            "auto-start ligado: ao abrir a TUI, o agente é iniciado se estiver parado",
        );
        if !app.live && app.agent.starting.is_none() {
            actions::start_agent(app, tx);
        }
    } else {
        app.notify(
            Level::Info,
            "auto-start desligado: abrir a TUI não inicia mais o agente",
        );
    }
}

fn open_rename_node(app: &mut App) {
    let current = app
        .status
        .as_ref()
        .and_then(|status| status.node_name.clone())
        .or_else(|| app.node.as_ref().map(|node| node.node_name.clone()))
        .unwrap_or_default();
    app.form = Some(app::rename_node_form(&current));
}

fn open_new_share(app: &mut App) {
    if !app.is_enrolled() {
        app.notify(Level::Warn, "entre na malha antes de publicar um serviço");
        return;
    }
    app.form = Some(app::new_share_form());
}

fn navigate(app: &mut App, down: bool) {
    match app.tab {
        TAB_PEERS => select(&mut app.table, app.peers.len(), down, 1),
        TAB_SHARES => {
            select(&mut app.shares_table, app.shares.len(), down, 1);
            app.delete_armed = None;
        }
        TAB_SERVICES => select(&mut app.jobs_table, app.jobs.len(), down, 1),
        TAB_CONSOLE => app.scroll_console(if down { -1 } else { 1 }),
        TAB_HELP => {
            if down {
                app.help_scroll = app.help_scroll.saturating_add(1);
            } else {
                app.help_scroll = app.help_scroll.saturating_sub(1);
            }
        }
        _ => {}
    }
}

fn navigate_by(app: &mut App, delta: isize) {
    match app.tab {
        TAB_PEERS => select_step(&mut app.table, app.peers.len(), delta),
        TAB_SHARES => {
            select_step(&mut app.shares_table, app.shares.len(), delta);
            app.delete_armed = None;
        }
        TAB_SERVICES => select_step(&mut app.jobs_table, app.jobs.len(), delta),
        TAB_CONSOLE => app.scroll_console(-delta),
        TAB_HELP => {
            if delta > 0 {
                app.help_scroll = app.help_scroll.saturating_add(delta as usize);
            } else {
                app.help_scroll = app.help_scroll.saturating_sub((-delta) as usize);
            }
        }
        _ => {}
    }
}

fn select(table: &mut ratatui::widgets::TableState, len: usize, down: bool, step: usize) {
    select_step(table, len, if down { step as isize } else { -(step as isize) });
}

fn select_step(table: &mut ratatui::widgets::TableState, len: usize, delta: isize) {
    if len == 0 {
        table.select(None);
        return;
    }
    let current = table.selected().unwrap_or(0) as isize;
    let next = (current + delta).clamp(0, len as isize - 1);
    table.select(Some(next as usize));
}

fn handle_form_key(app: &mut App, key: KeyEvent, tx: &Sender<Msg>) {
    match key.code {
        KeyCode::Esc => {
            app.form = None;
            return;
        }
        KeyCode::Enter => {
            actions::submit_form(app, tx);
            return;
        }
        _ => {}
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        if key.code == KeyCode::Char('u') {
            if let Some(form) = app.form.as_mut() {
                if let Some(field) = form.fields.get_mut(form.focus) {
                    if !matches!(field.kind, FieldKind::Choice | FieldKind::Bool) {
                        field.value.clear();
                    }
                }
                form.error = None;
            }
        }
        return;
    }
    let Some(form) = app.form.as_mut() else {
        return;
    };
    match key.code {
        KeyCode::Tab | KeyCode::Down => form.next_field(),
        KeyCode::BackTab | KeyCode::Up => form.previous_field(),
        KeyCode::Left => {
            if let Some(field) = form.fields.get_mut(form.focus) {
                if matches!(field.kind, FieldKind::Choice | FieldKind::Bool) {
                    field.cycle(false);
                }
            }
            form.error = None;
        }
        KeyCode::Right => {
            if let Some(field) = form.fields.get_mut(form.focus) {
                if matches!(field.kind, FieldKind::Choice | FieldKind::Bool) {
                    field.cycle(true);
                }
            }
            form.error = None;
        }
        KeyCode::Backspace => {
            if let Some(field) = form.fields.get_mut(form.focus) {
                if !matches!(field.kind, FieldKind::Choice | FieldKind::Bool) {
                    field.value.pop();
                }
            }
            form.error = None;
        }
        KeyCode::Char(' ') => {
            if let Some(field) = form.fields.get_mut(form.focus) {
                if field.kind == FieldKind::Bool {
                    field.cycle(true);
                } else if field.value.chars().count() < field.max {
                    field.value.push(' ');
                }
            }
            form.error = None;
        }
        KeyCode::Char(ch) => {
            if let Some(field) = form.fields.get_mut(form.focus) {
                let editable = !matches!(field.kind, FieldKind::Choice | FieldKind::Bool);
                let allowed = match field.kind {
                    FieldKind::Digits => ch.is_ascii_digit(),
                    _ => !ch.is_control(),
                };
                if editable && allowed && field.value.chars().count() < field.max {
                    field.value.push(ch);
                }
            }
            form.error = None;
        }
        _ => {}
    }
}

fn handle_confirm_key(app: &mut App, key: KeyEvent, tx: &Sender<Msg>) {
    match key.code {
        KeyCode::Char('s') | KeyCode::Char('S') => {
            if let Some(confirm) = app.confirm.take() {
                actions::perform_confirm(app, confirm.action, tx);
            }
        }
        KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
            app.confirm = None;
        }
        _ => {}
    }
}

fn handle_input_key(app: &mut App, key: KeyEvent, tx: &Sender<Msg>) {
    match key.code {
        KeyCode::Esc => {
            app.input = None;
            return;
        }
        KeyCode::Enter => {
            actions::submit_console(app, tx);
            return;
        }
        KeyCode::Tab => {
            let Some(input) = app.input.as_mut() else {
                return;
            };
            let value = input.value.clone();
            if value.contains(char::is_whitespace) {
                return;
            }
            if let Some(first) = command::suggestions(&value).first() {
                input.value = first.to_string();
                input.error = None;
            }
            return;
        }
        KeyCode::Up => {
            history_move(app, true);
            return;
        }
        KeyCode::Down => {
            history_move(app, false);
            return;
        }
        _ => {}
    }
    let Some(input) = app.input.as_mut() else {
        return;
    };
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        if key.code == KeyCode::Char('u') {
            input.value.clear();
            input.error = None;
        }
        return;
    }
    match key.code {
        KeyCode::Backspace => {
            input.value.pop();
            input.error = None;
        }
        KeyCode::Char(ch) if !ch.is_control() && input.value.chars().count() < 512 => {
            input.value.push(ch);
            input.error = None;
        }
        _ => {}
    }
}

fn history_move(app: &mut App, up: bool) {
    let history = app.history.clone();
    let Some(input) = app.input.as_mut() else {
        return;
    };
    if history.is_empty() {
        return;
    }
    if up {
        let index = match input.history_index {
            Some(0) => 0,
            Some(index) => index - 1,
            None => history.len() - 1,
        };
        input.history_index = Some(index);
        input.value = history[index].clone();
    } else {
        match input.history_index {
            Some(index) if index + 1 < history.len() => {
                input.history_index = Some(index + 1);
                input.value = history[index + 1].clone();
            }
            Some(_) | None => {
                input.history_index = None;
                input.value.clear();
            }
        }
    }
    input.error = None;
}

fn print_summary(app: &App, saved: bool) {
    let mut out = std::io::stdout();
    let _ = writeln!(out);
    let root = format!("'{}'", app.root.display().to_string().replace('\'', "'\"'\"'"));
    let live = agent::socket_ready(&app.socket);
    if live {
        let pid = agent::read_pid(&app.root)
            .or(app.agent.pid)
            .map(|pid| format!(" (pid {pid})"))
            .unwrap_or_default();
        let _ = writeln!(out, "O agente continua ativo{pid} em segundo plano.");
        let _ = writeln!(out, "  reabrir:  wm --data-dir {root} tui");
        let _ = writeln!(out, "  parar:    wm --data-dir {root} down");
        let _ = writeln!(out, "  log:      {}", app.log_path.display());
    } else if app.agent.starting.is_some() {
        let _ = writeln!(
            out,
            "O agente foi iniciado em segundo plano e continua subindo."
        );
        let _ = writeln!(out, "  confira com: wm --data-dir {root} status");
    } else if app.is_enrolled() {
        let _ = writeln!(
            out,
            "O agente está parado. A inscrição, os shares e as configurações continuam salvos."
        );
        let _ = writeln!(
            out,
            "  reabrir e iniciar: wm --data-dir {root} tui   (ou pressione s na TUI)"
        );
    } else {
        let _ = writeln!(
            out,
            "O agente não foi iniciado nesta TUI. Use wm --data-dir {root} tui para entrar na malha."
        );
    }
    let jobs: Vec<&app::Job> = app
        .jobs
        .iter()
        .filter(|job| agent::pid_alive(job.pid))
        .collect();
    if !jobs.is_empty() {
        let _ = writeln!(out, "Também continua em segundo plano:");
        for job in jobs {
            let _ = writeln!(
                out,
                "  {} (pid {}) · log {}",
                job.label,
                job.pid,
                job.log.display()
            );
        }
    }
    if saved {
        let _ = writeln!(
            out,
            "Sessão salva em {} (aba, histórico e último retrato).",
            persist::state_path(&app.root).display()
        );
    } else {
        let _ = writeln!(
            out,
            "Atenção: não foi possível salvar {} (permissão?). O agente não depende disso.",
            persist::state_path(&app.root).display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "wm-tui-lib-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn sender() -> (Sender<Msg>, Receiver<Msg>) {
        mpsc::channel()
    }

    fn press(app: &mut App, code: KeyCode, tx: &Sender<Msg>) {
        handle_key(app, KeyEvent::new(code, KeyModifiers::NONE), tx);
    }

    fn type_text(app: &mut App, text: &str, tx: &Sender<Msg>) {
        for ch in text.chars() {
            press(app, KeyCode::Char(ch), tx);
        }
    }

    #[test]
    fn q_and_ctrl_c_quit_without_stopping_anything() {
        let root = temp_root("quit");
        let mut app = App::load(root.clone());
        let (tx, _rx) = sender();
        press(&mut app, KeyCode::Char('q'), &tx);
        assert!(app.quit);
        assert!(app.agent.stopping.is_none());

        let mut app = App::load(root.clone());
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &tx,
        );
        assert!(app.quit);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tab_numbers_help_and_join_shortcut() {
        let root = temp_root("keys");
        let mut app = App::load(root.clone());
        let (tx, _rx) = sender();
        press(&mut app, KeyCode::Char('5'), &tx);
        assert_eq!(app.tab, TAB_AGENT);
        press(&mut app, KeyCode::Char('7'), &tx);
        assert_eq!(app.tab, TAB_CONSOLE);
        press(&mut app, KeyCode::Char('?'), &tx);
        assert_eq!(app.tab, TAB_HELP);

        app.tab = TAB_STATUS;
        press(&mut app, KeyCode::Char('j'), &tx);
        let form = app.form.as_ref().expect("join form");
        assert_eq!(form.action, app::FormAction::Join);
        press(&mut app, KeyCode::Esc, &tx);
        assert!(app.form.is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn console_keeps_input_and_error_for_invalid_command() {
        let root = temp_root("console");
        let mut app = App::load(root.clone());
        let (tx, _rx) = sender();
        press(&mut app, KeyCode::Char(':'), &tx);
        assert!(app.input.is_some());
        type_text(&mut app, "tui", &tx);
        press(&mut app, KeyCode::Enter, &tx);
        let input = app.input.as_ref().expect("input should stay open");
        assert_eq!(input.value, "tui");
        assert!(input.error.is_some());
        assert_eq!(app.tab, TAB_CONSOLE);

        app.input = None;
        press(&mut app, KeyCode::Char(':'), &tx);
        type_text(&mut app, "shares", &tx);
        press(&mut app, KeyCode::Enter, &tx);
        assert!(app.input.is_none());
        assert_eq!(app.tab, TAB_SHARES);
        assert!(app.history.iter().any(|line| line == "shares"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn agent_settings_form_is_applied_and_persisted() {
        let root = temp_root("settings");
        let mut app = App::load(root.clone());
        let (tx, _rx) = sender();
        app.tab = TAB_AGENT;
        press(&mut app, KeyCode::Char('e'), &tx);
        let form = app.form.as_mut().expect("settings form");
        form.fields[0].value = "12345".into();
        form.fields[1].value = "1400".into();
        form.fields[3].value = "8421:3000".into();
        actions::submit_form(&mut app, &tx);
        assert!(app.form.is_none());
        assert_eq!(app.settings.udp_port, 12345);
        assert_eq!(app.settings.mtu, 1400);
        assert_eq!(app.settings.expose, "8421:3000");
        assert!(app.save());
        let restored = App::load(root.clone());
        assert_eq!(restored.settings.udp_port, 12345);
        assert_eq!(restored.settings.expose, "8421:3000");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stop_is_reported_once_even_if_two_events_arrive() {
        use crate::agent::AgentEvent;
        let root = temp_root("stop");
        let mut app = App::load(root.clone());
        let (tx, _rx) = sender();
        app.agent.stopping = Some(Instant::now());
        let before = app.notices.len();
        handle_agent_event(
            &mut app,
            AgentEvent::Exited {
                pid: 4242,
                code: Some(0),
            },
            &tx,
        );
        let after_exit = app.notices.len();
        assert_eq!(after_exit, before + 1);
        assert!(!app.auto_start);
        assert_eq!(app.agent.last_stop_pid, Some(4242));
        handle_agent_event(&mut app, AgentEvent::Stopped { pid: 4242 }, &tx);
        assert_eq!(app.notices.len(), after_exit, "Stopped duplica o aviso");
        assert_eq!(app.agent.last_stop_pid, None);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stop_events_are_idempotent_in_both_orders() {
        use crate::agent::AgentEvent;
        let root = temp_root("stop-order");
        let mut app = App::load(root.clone());
        let (tx, _rx) = sender();
        app.agent.stopping = Some(Instant::now());
        let before = app.notices.len();
        handle_agent_event(&mut app, AgentEvent::Stopped { pid: 99 }, &tx);
        assert_eq!(app.notices.len(), before + 1);
        handle_agent_event(
            &mut app,
            AgentEvent::Exited {
                pid: 99,
                code: Some(0),
            },
            &tx,
        );
        assert_eq!(app.notices.len(), before + 1, "Exited duplica o aviso");
        assert_eq!(app.agent.last_stop_pid, None);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unexpected_exit_is_warned_and_clears_pid() {
        use crate::agent::AgentEvent;
        let root = temp_root("crash");
        let mut app = App::load(root.clone());
        let (tx, _rx) = sender();
        app.agent.pid = Some(777);
        app.agent.managed = true;
        handle_agent_event(
            &mut app,
            AgentEvent::Exited {
                pid: 777,
                code: Some(1),
            },
            &tx,
        );
        assert_eq!(app.agent.pid, None);
        assert!(!app.live);
        assert!(app
            .last_notice()
            .is_some_and(|notice| notice.level == Level::Warn));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fetcher_reads_status_peers_shares_extensions_and_log() {
        use serde_json::json;
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixListener;
        use uuid::Uuid;
        use wm_proto::{
            AgentStatus, ExtensionState, ExtensionStatus, NatType, PathKind, PeerStatus, ShareInfo,
            ShareKind, ShareProtocol,
        };

        let root = temp_root("fetch");
        let socket = PathBuf::from(format!(
            "/tmp/wm-tui-fetch-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let node_id = Uuid::new_v4();
        let status = AgentStatus {
            version: "0.1.0".into(),
            node_id: Some(node_id),
            node_name: Some("site".into()),
            overlay_ip: Some("10.66.0.1".parse().unwrap()),
            control_url: Some("https://mesh.test".into()),
            control_connected: true,
            relay_connected: false,
            tun_name: Some("utun7".into()),
            udp_port: Some(41641),
            nat_type: NatType::Cone,
            peers_total: 1,
            peers_online: 1,
            uptime_secs: 10,
            map_version: 2,
            last_error: None,
        };
        let peer = PeerStatus {
            id: Uuid::new_v4(),
            name: "mac-01".into(),
            overlay_ip: "10.66.0.12".parse().unwrap(),
            path: PathKind::Direct,
            endpoint: Some("203.0.113.9:41641".into()),
            rtt_ms: Some(4),
            last_handshake_secs: Some(2),
            rx_bytes: 10,
            tx_bytes: 20,
            relay_online: true,
            last_error: None,
        };
        let share = ShareInfo {
            id: Uuid::new_v4(),
            name: "demo".into(),
            node_id,
            node_name: "site".into(),
            overlay_ip: "10.66.0.1".parse().unwrap(),
            port: 3000,
            kind: ShareKind::Web,
            protocol: ShareProtocol::Tcp,
            description: String::new(),
            enabled: true,
            online: true,
            public_url: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let extension = ExtensionStatus {
            id: "cloudflared".into(),
            name: "tunnel".into(),
            driver: "cloudflared".into(),
            state: ExtensionState::Running,
            message: None,
            public_urls: vec!["https://demo.example".into()],
            details: Value::Null,
            updated_at: Utc::now(),
        };
        let listener = UnixListener::bind(&socket).unwrap();
        let responses = vec![
            json!({"id":1,"ok":true,"result": status}),
            json!({"id":1,"ok":true,"result": [peer]}),
            json!({"id":1,"ok":true,"result": [share]}),
            json!({"id":1,"ok":true,"result": [extension]}),
        ];
        let server = {
            let socket = socket.clone();
            std::thread::spawn(move || {
                for response in responses {
                    let Ok((mut stream, _)) = listener.accept() else {
                        break;
                    };
                    let mut line = String::new();
                    let mut reader = BufReader::new(&mut stream);
                    if reader.read_line(&mut line).is_err() {
                        break;
                    }
                    let _ = stream.write_all(format!("{response}\n").as_bytes());
                    let _ = stream.flush();
                }
                let _ = std::fs::remove_file(&socket);
            })
        };
        std::fs::write(
            root.join("agent.log"),
            "\u{1b}[32mINFO\u{1b}[0m agente pronto\n",
        )
        .unwrap();
        let (tx, rx) = sender();
        start_fetcher(root.clone(), socket.clone(), root.join("agent.log"), tx);
        let snapshot = loop {
            match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(Msg::Update(snapshot)) => break snapshot,
                Ok(_) => continue,
                Err(err) => panic!("sem snapshot: {err}"),
            }
        };
        assert_eq!(snapshot.status.as_ref().unwrap().node_name.as_deref(), Some("site"));
        assert_eq!(snapshot.peers.len(), 1);
        assert_eq!(snapshot.shares.len(), 1);
        assert_eq!(snapshot.extensions.len(), 1);
        assert!(snapshot.error.is_none());
        assert!(snapshot.log[0].contains("agente pronto"));
        assert_eq!(server.join().is_ok(), true);
        let mut app = App::load(root.clone());
        app.apply_snapshot(snapshot);
        assert_eq!(app.agent.log, vec!["INFO agente pronto"]);
        assert_eq!(app.node.as_ref().unwrap().node_name, "site");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn job_log_viewer_switches_to_console_and_sanitizes() {
        let root = temp_root("joblog");
        let mut app = App::load(root.clone());
        let (tx, _rx) = sender();
        let log = root.join("gui.log");
        std::fs::write(&log, "linha \u{1b}[31mvermelha\u{1b}[0m\nsegunda\n").unwrap();
        app.add_job(std::process::id(), "gui".into(), log);
        app.jobs_table.select(Some(0));
        app.tab = TAB_SERVICES;
        press(&mut app, KeyCode::Enter, &tx);
        assert_eq!(app.tab, TAB_CONSOLE);
        assert!(app.console.iter().any(|line| line.text == "linha vermelha"));
        assert!(!app
            .console
            .iter()
            .any(|line| line.text.contains('\u{1b}')));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn console_transcript_scrolls_and_is_bounded() {
        let root = temp_root("scroll");
        let mut app = App::load(root.clone());
        let (tx, _rx) = sender();
        app.tab = TAB_CONSOLE;
        for index in 0..20 {
            app.push_console(Level::Output, format!("linha {index}"));
        }
        press(&mut app, KeyCode::Char('k'), &tx);
        assert_eq!(app.console_scroll, 1);
        press(&mut app, KeyCode::End, &tx);
        assert_eq!(app.console_scroll, 0);
        std::fs::remove_dir_all(root).unwrap();
    }
}
