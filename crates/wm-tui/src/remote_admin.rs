//! Standalone remote administration console. Integration: expose this module and
//! call `remote_admin::run(server)` from `wm admin tui` (outside any active TUI).
//!
//! No local agent, server, shell, persistence or credential logging is used.
//! All SDK calls, including login/logout, belong to one bounded worker. UI errors
//! are deliberately static: server error bodies/URLs may contain credentials.

use std::collections::VecDeque;
use std::io;
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::thread;
use std::time::Duration;

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::cursor::Show;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use serde_json::Value;
use uuid::Uuid;
use wm_ext::admin::{AdminClient, AdminClientOptions, AdminSession};
use wm_proto::{AclView, NodeView, Overview};

const MAX_FORM: usize = 64 * 1024;
const MAX_LOGIN: usize = 2048;
const MAX_RESPONSE: usize = 1024 * 1024;
const MAX_STATUS: usize = 64;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Overview,
    Nodes,
    Acl,
    Extensions,
    Status,
}
impl Page {
    fn index(self) -> usize {
        match self {
            Self::Overview => 0,
            Self::Nodes => 1,
            Self::Acl => 2,
            Self::Extensions => 3,
            Self::Status => 4,
        }
    }
    fn at(index: usize) -> Self {
        [Self::Overview, Self::Nodes, Self::Acl, Self::Extensions, Self::Status][index % 5]
    }
}

// Do not derive Debug for types that can hold credentials or configuration.
enum Command {
    Login { server: String, user: String, password: String, insecure: bool },
    Fetch(Page),
    Approve(Uuid),
    Revoke(Uuid),
    PutAcl(String),
    Reload,
    Config(String, Uuid),
    PutConfig(String, Uuid, bool, Value),
    Activate(String, Uuid, bool),
}

enum Reply {
    LoggedIn,
    Overview(Overview),
    Nodes(Vec<NodeView>),
    Acl(AclView),
    Extensions(Value, Vec<NodeView>),
    Config(String, Uuid, Value),
    Changed(&'static str),
    Failed(&'static str),
}

/// Only one request can be in flight. The session never leaves this thread.
fn worker(rx: Receiver<Command>, tx: SyncSender<Reply>) {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
        let _ = tx.send(Reply::Failed("Não foi possível iniciar o worker."));
        return;
    };
    let mut connection: Option<(AdminClient, AdminSession)> = None;
    while let Ok(command) = rx.recv() {
        let reply = runtime.block_on(async {
            // Also bound multi-call operations, independently of SDK timeouts.
            let outcome = tokio::time::timeout(REQUEST_TIMEOUT, async {
                match command {
                    Command::Login { server, user, mut password, insecure } => {
                        // The SDK permits loopback HTTP by default; this console
                        // deliberately requires explicit consent for ALL plaintext.
                        if server.to_ascii_lowercase().starts_with("http://") && !insecure {
                            return Err("HTTP exige opt-in explícito, inclusive em loopback.");
                        }
                        let options = AdminClientOptions {
                            allow_insecure_http_overlay: insecure,
                            timeout: Duration::from_secs(10),
                            max_response_bytes: MAX_RESPONSE,
                            ..Default::default()
                        };
                        let client = AdminClient::with_options(&server, options)
                            .map_err(|_| "Servidor inválido: use HTTPS; HTTP overlay exige opt-in explícito.")?;
                        let login = client.login(&user, &password).await;
                        // Remove the owned password immediately after the request. No
                        // guarantee of allocator-level zeroization is made by String.
                        password.clear();
                        let session = login.map_err(|_| "Login falhou. Verifique servidor, credenciais e TLS.")?;
                        connection = Some((client, session));
                        return Ok(Reply::LoggedIn);
                    }
                    command => {
                        let (client, session) = connection.as_ref().ok_or("Entre novamente para continuar.")?;
                        let failed = |_| "Requisição falhou; verifique conexão/permissões ou saia e entre novamente.";
                        Ok(match command {
                            Command::Fetch(Page::Overview) => Reply::Overview(client.overview(session).await.map_err(failed)?),
                            Command::Fetch(Page::Nodes) => Reply::Nodes(client.nodes(session).await.map_err(failed)?),
                            Command::Fetch(Page::Acl) => Reply::Acl(client.acl(session).await.map_err(failed)?),
                            Command::Fetch(Page::Extensions) => {
                                let extensions = client.extensions(session).await.map_err(failed)?;
                                let nodes = client.nodes(session).await.map_err(failed)?;
                                Reply::Extensions(extensions, nodes)
                            }
                            Command::Fetch(Page::Status) => Reply::Changed("Status é local; API de logs não faz parte do SDK."),
                            Command::Approve(id) => {
                                client.approve_node(session, id).await.map_err(failed)?;
                                Reply::Changed("Nó aprovado. Atualize a lista com r.")
                            }
                            Command::Revoke(id) => {
                                client.revoke_node(session, id).await.map_err(failed)?;
                                Reply::Changed("Nó revogado. Atualize a lista com r.")
                            }
                            Command::PutAcl(text) => Reply::Acl(client.put_acl(session, &text).await.map_err(failed)?),
                            Command::Reload => {
                                client.reload_extensions(session).await.map_err(failed)?;
                                Reply::Changed("Extensões recarregadas. Atualize a lista com r.")
                            }
                            Command::Config(id, node) => {
                                let value = client.extension_config(session, &id, node).await.map_err(failed)?;
                                Reply::Config(id, node, value)
                            }
                            Command::PutConfig(id, node, enabled, value) => {
                                let value = client.put_extension_config(session, &id, node, enabled, &value).await.map_err(failed)?;
                                Reply::Config(id, node, value)
                            }
                            Command::Activate(id, node, active) => {
                                client.set_extension_activation(session, &id, node, active).await.map_err(failed)?;
                                Reply::Changed("Ativação atualizada. Enter recarrega configuração do nó.")
                            }
                            Command::Login { .. } => unreachable!(),
                        })
                    }
                }
            }).await;
            match outcome {
                Ok(Ok(reply)) => reply,
                Ok(Err(message)) => Reply::Failed(message),
                Err(_) => Reply::Failed("Tempo limite excedido. Escrita pode ter ocorrido; atualize antes de repetir."),
            }
        });
        if tx.send(reply).is_err() {
            break;
        }
    }
    if let Some((client, session)) = connection {
        runtime.block_on(async {
            let _ = tokio::time::timeout(Duration::from_secs(3), client.logout(&session)).await;
        });
    }
    // A cancelled DNS lookup may still own a Tokio blocking task. Do not let
    // Runtime::drop wait indefinitely for it after the terminal is restored.
    runtime.shutdown_timeout(Duration::from_secs(1));
}

struct Login {
    server: String,
    user: String,
    password: String,
    field: usize,
    insecure: bool,
}
impl Login {
    fn new(server: Option<String>) -> Self {
        Self { server: server.unwrap_or_default(), user: "admin".into(), password: String::new(), field: 0, insecure: false }
    }
    fn masked_password(&self) -> String {
        "*".repeat(self.password.chars().count())
    }
    fn input(&mut self) -> Option<&mut String> {
        match self.field { 0 => Some(&mut self.server), 1 => Some(&mut self.user), 2 => Some(&mut self.password), _ => None }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Target {
    Acl,
    Config(String, Uuid, bool),
}
struct Editor {
    target: Target,
    text: String,
    cursor: usize,
}
impl Editor {
    fn new(target: Target, text: String) -> Option<Self> {
        (text.len() <= MAX_FORM).then(|| Self { cursor: text.len(), target, text })
    }
    fn insert(&mut self, ch: char) {
        if self.text.len() + ch.len_utf8() <= MAX_FORM {
            self.text.insert(self.cursor, ch);
            self.cursor += ch.len_utf8();
        }
    }
    fn left(&mut self) {
        self.cursor = self.text[..self.cursor].char_indices().next_back().map(|(i, _)| i).unwrap_or(0);
    }
    fn right(&mut self) {
        if let Some(ch) = self.text[self.cursor..].chars().next() { self.cursor += ch.len_utf8(); }
    }
    fn backspace(&mut self) {
        let end = self.cursor;
        self.left();
        self.text.drain(self.cursor..end);
    }
    fn delete(&mut self) {
        if let Some(ch) = self.text[self.cursor..].chars().next() {
            self.text.drain(self.cursor..self.cursor + ch.len_utf8());
        }
    }
    fn vertical(&mut self, down: bool) {
        let start = self.text[..self.cursor].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let col = self.text[start..self.cursor].chars().count();
        let next_start = if down {
            self.text[self.cursor..].find('\n').map(|i| self.cursor + i + 1)
        } else if start > 0 {
            Some(self.text[..start - 1].rfind('\n').map(|i| i + 1).unwrap_or(0))
        } else { None };
        if let Some(next_start) = next_start {
            let line = self.text[next_start..].split('\n').next().unwrap_or("");
            let offset = line.char_indices().nth(col).map(|(i, _)| i).unwrap_or(line.len());
            self.cursor = next_start + offset;
        }
    }
}

struct Confirmation {
    label: String,
    command: Command,
    // One explicit affirmative key is required, never Enter/default acceptance.
}
impl Confirmation {
    fn resolve(self, key: KeyCode) -> Option<Command> {
        if key == KeyCode::Char('y') { Some(self.command) } else { None }
    }
}

struct App {
    login: Login,
    authenticated: bool,
    busy: bool,
    page: Page,
    overview: Option<Overview>,
    nodes: Vec<NodeView>,
    selected_node: usize,
    node_detail: bool,
    acl: Option<AclView>,
    extensions: Vec<Value>,
    selected_extension: usize,
    config: Option<(String, Uuid, Value)>,
    editor: Option<Editor>,
    confirm: Option<Confirmation>,
    status: VecDeque<&'static str>,
    scroll: u16,
}
impl App {
    fn new(server: Option<String>) -> Self {
        Self {
            login: Login::new(server), authenticated: false, busy: false, page: Page::Overview,
            overview: None, nodes: Vec::new(), selected_node: 0, node_detail: false,
            acl: None, extensions: Vec::new(), selected_extension: 0, config: None,
            editor: None, confirm: None, status: VecDeque::new(), scroll: 0,
        }
    }
    fn note(&mut self, message: &'static str) {
        if self.status.len() == MAX_STATUS { self.status.pop_front(); }
        self.status.push_back(message);
    }
    fn send(&mut self, tx: &SyncSender<Command>, command: Command) {
        if self.busy { return; }
        match tx.try_send(command) {
            Ok(()) => { self.busy = true; self.note("Requisição em andamento…"); }
            Err(_) => self.note("Worker indisponível; saia e tente novamente."),
        }
    }
    fn receive(&mut self, reply: Reply) {
        self.busy = false;
        match reply {
            Reply::LoggedIn => { self.authenticated = true; self.login.password.clear(); self.note("Sessão aberta (somente memória)."); }
            Reply::Overview(value) => { self.overview = Some(value); self.note("Overview atualizado."); }
            Reply::Nodes(value) => { self.nodes = value; self.clamp_selection(); self.note("Nós atualizados."); }
            Reply::Acl(value) => { self.acl = Some(value); self.note("ACL atualizada."); }
            Reply::Extensions(value, nodes) => {
                self.extensions = value.as_array().cloned().unwrap_or_default();
                // A refresh can reorder/remove nodes/extensions. Never retain a
                // detail form whose visible selection now identifies another target.
                self.config = None;
                self.nodes = nodes;
                self.clamp_selection();
                self.note("Extensões atualizadas.");
            }
            Reply::Config(id, node, value) => { self.config = Some((id, node, value)); self.scroll = 0; self.note("Configuração atualizada."); }
            Reply::Changed(message) | Reply::Failed(message) => self.note(message),
        }
    }
    fn clamp_selection(&mut self) {
        self.selected_node = self.selected_node.min(self.nodes.len().saturating_sub(1));
        self.selected_extension = self.selected_extension.min(self.extensions.len().saturating_sub(1));
    }
    fn node_id(&self) -> Option<Uuid> { self.nodes.get(self.selected_node).map(|n| n.node.id) }
    fn extension_id(&self) -> Option<String> {
        self.extensions.get(self.selected_extension)?.get("id")?.as_str().map(str::to_owned)
    }
    fn confirm(&mut self, label: String, command: Command) {
        self.confirm = Some(Confirmation { label, command });
    }
    fn open_editor(&mut self, target: Target, value: &Value) {
        let text = serde_json::to_string_pretty(value).unwrap_or_default();
        self.editor = Editor::new(target, text);
        if self.editor.is_none() { self.note("Documento excede o limite do formulário (64 KiB)."); }
    }
    fn change_page(&mut self, page: Page, tx: &SyncSender<Command>) {
        self.page = page;
        self.scroll = 0;
        self.config = None;
        self.node_detail = false;
        if page != Page::Status { self.send(tx, Command::Fetch(page)); }
    }
    // Return true only when the user asks to leave. No persistence on this path.
    fn key(&mut self, key: KeyEvent, tx: &SyncSender<Command>) -> bool {
        if key.kind == KeyEventKind::Release { return false; }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) { return true; }
        if self.busy {
            return matches!(key.code, KeyCode::Esc) || (self.editor.is_none() && self.authenticated && key.code == KeyCode::Char('q'));
        }
        if let Some(confirm) = self.confirm.take() {
            // Any other key cancels, including Enter, Esc, uppercase Y and q.
            if let Some(command) = confirm.resolve(key.code) { self.send(tx, command); }
            return false;
        }
        if self.editor.is_some() {
            self.editor_key(key);
            return false;
        }
        if !self.authenticated {
            match key.code {
                KeyCode::Esc => return true,
                KeyCode::Tab | KeyCode::Down => self.login.field = (self.login.field + 1) % 4,
                KeyCode::BackTab | KeyCode::Up => self.login.field = (self.login.field + 3) % 4,
                KeyCode::Char(' ') if self.login.field == 3 => self.login.insecure = !self.login.insecure,
                KeyCode::Enter => {
                    if self.login.server.trim().is_empty() || self.login.user.trim().is_empty() || self.login.password.is_empty() {
                        self.note("Informe servidor explícito, usuário e senha.");
                    } else {
                        let command = Command::Login {
                            server: self.login.server.trim().into(), user: self.login.user.clone(),
                            password: std::mem::take(&mut self.login.password), insecure: self.login.insecure,
                        };
                        self.send(tx, command);
                    }
                }
                KeyCode::Backspace => { if let Some(input) = self.login.input() { input.pop(); } }
                KeyCode::Char(ch) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) && !ch.is_control() => {
                    if let Some(input) = self.login.input() {
                        if input.len() + ch.len_utf8() <= MAX_LOGIN { input.push(ch); }
                    }
                }
                _ => {}
            }
            return false;
        }
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Char(ch @ '1'..='5') => self.change_page(Page::at(ch as usize - '1' as usize), tx),
            KeyCode::Tab | KeyCode::Right => self.change_page(Page::at(self.page.index() + 1), tx),
            KeyCode::BackTab | KeyCode::Left => self.change_page(Page::at(self.page.index() + 4), tx),
            KeyCode::Esc => { self.config = None; self.node_detail = false; self.scroll = 0; }
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(10),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::Char('r') => self.send(tx, Command::Fetch(self.page)),
            KeyCode::Down if self.page == Page::Nodes && !self.node_detail => {
                self.selected_node = (self.selected_node + 1).min(self.nodes.len().saturating_sub(1));
            }
            KeyCode::Up if self.page == Page::Nodes && !self.node_detail => self.selected_node = self.selected_node.saturating_sub(1),
            KeyCode::Down if self.page == Page::Extensions && self.config.is_none() => {
                self.selected_extension = (self.selected_extension + 1).min(self.extensions.len().saturating_sub(1));
            }
            KeyCode::Up if self.page == Page::Extensions && self.config.is_none() => self.selected_extension = self.selected_extension.saturating_sub(1),
            KeyCode::Down => self.scroll = self.scroll.saturating_add(1),
            KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Char('n') if self.page == Page::Extensions => {
                if !self.nodes.is_empty() { self.selected_node = (self.selected_node + 1) % self.nodes.len(); }
                self.config = None;
            }
            KeyCode::Enter if self.page == Page::Nodes => { self.node_detail = !self.node_detail; self.scroll = 0; }
            KeyCode::Enter if self.page == Page::Extensions => {
                if let (Some(id), Some(node)) = (self.extension_id(), self.node_id()) { self.send(tx, Command::Config(id, node)); }
            }
            KeyCode::Char('a') | KeyCode::Char('d') if self.page == Page::Nodes => {
                if let Some(id) = self.node_id() {
                    let approve = key.code == KeyCode::Char('a');
                    self.confirm(format!("{} nó {id}?", if approve { "Aprovar" } else { "REVOGAR" }), if approve { Command::Approve(id) } else { Command::Revoke(id) });
                }
            }
            KeyCode::Char('e') if self.page == Page::Acl => {
                if let Some(acl) = &self.acl {
                    let value = serde_json::to_value(&acl.policy).unwrap_or(Value::Null);
                    self.open_editor(Target::Acl, &value);
                }
            }
            KeyCode::Char('e') if self.page == Page::Extensions => {
                if let Some((id, node, value)) = &self.config {
                    let target = Target::Config(id.clone(), *node, value["enabled"].as_bool().unwrap_or(false));
                    let config = value.get("config").cloned().unwrap_or_else(|| serde_json::json!({}));
                    self.open_editor(target, &config);
                }
            }
            KeyCode::Char('a') | KeyCode::Char('d') if self.page == Page::Extensions => {
                if let (Some(id), Some(node)) = (self.extension_id(), self.node_id()) {
                    let active = key.code == KeyCode::Char('a');
                    self.confirm(format!("{} {} no nó {node}?", if active { "Ativar" } else { "Desativar" }, safe_text(&id)), Command::Activate(id, node, active));
                }
            }
            KeyCode::Char('R') if self.page == Page::Extensions => self.confirm("Recarregar manifests no servidor?".into(), Command::Reload),
            _ => {}
        }
        false
    }
    fn editor_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Esc { self.editor = None; return; }
        if key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL) {
            let editor = self.editor.as_ref().unwrap();
            let Ok(value) = serde_json::from_str::<Value>(&editor.text) else {
                self.note("JSON inválido; corrija antes de salvar."); return;
            };
            let editor = self.editor.take().unwrap();
            let (label, command) = match editor.target {
                Target::Acl => ("Substituir política ACL no servidor?".into(), Command::PutAcl(editor.text)),
                Target::Config(id, node, enabled) => (
                    format!("Salvar configuração de {} no nó {node} (enabled={enabled})?", safe_text(&id)),
                    Command::PutConfig(id, node, enabled, value),
                ),
            };
            self.confirm(label, command);
            return;
        }
        let editor = self.editor.as_mut().unwrap();
        match key.code {
            KeyCode::F(2) => if let Target::Config(_, _, enabled) = &mut editor.target { *enabled = !*enabled; },
            KeyCode::Left => editor.left(),
            KeyCode::Right => editor.right(),
            KeyCode::Up => editor.vertical(false),
            KeyCode::Down => editor.vertical(true),
            KeyCode::Home => editor.cursor = editor.text[..editor.cursor].rfind('\n').map(|i| i + 1).unwrap_or(0),
            KeyCode::End => editor.cursor += editor.text[editor.cursor..].find('\n').unwrap_or(editor.text.len() - editor.cursor),
            KeyCode::Backspace => editor.backspace(),
            KeyCode::Delete => editor.delete(),
            KeyCode::Enter => editor.insert('\n'),
            KeyCode::Tab => { editor.insert(' '); editor.insert(' '); }
            KeyCode::Char(ch) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) && !ch.is_control() => editor.insert(ch),
            _ => {}
        }
    }
}

fn safe_text(text: &str) -> String {
    text.chars().map(|ch| if ch.is_control() { ' ' } else { ch }).collect()
}
fn sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    ["password", "passwd", "secret", "token", "cookie", "csrf", "authorization", "credential", "private_key", "privatekey"]
        .iter().any(|part| key.contains(part))
}
fn redact(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(map.iter().map(|(key, value)| {
            (key.clone(), if sensitive_key(key) { Value::String("[redacted]".into()) } else { redact(value) })
        }).collect()),
        Value::Array(values) => Value::Array(values.iter().map(redact).collect()),
        value => value.clone(),
    }
}
fn json_text(value: &Value) -> String {
    serde_json::to_string_pretty(&redact(value)).unwrap_or_else(|_| "Não foi possível exibir JSON.".into())
}
fn node_label(node: &NodeView) -> String {
    let name = if node.node.display_name.is_empty() { &node.node.name } else { &node.node.display_name };
    format!("{}  {}  {:?}  {}", safe_text(name), node.node.overlay_ip, node.status, node.node.id)
}

fn render(frame: &mut Frame<'_>, app: &App) {
    let areas = Layout::default().direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(3), Constraint::Length(3)]).split(frame.area());
    let title = if app.authenticated {
        ["1 Overview", "2 Nós", "3 Acessos (ACL)", "4 Extensões", "5 Logs/status"].iter().enumerate()
            .map(|(i, name)| if i == app.page.index() { format!("[{name}]") } else { name.to_string() })
            .collect::<Vec<_>>().join(" | ")
    } else { "WM — administração remota / login".into() };
    frame.render_widget(Paragraph::new(title).block(Block::default().borders(Borders::ALL)), areas[0]);
    let mut scroll = app.scroll;
    let (body, help) = if let Some(confirm) = &app.confirm {
        (format!("{}\n\nPressione y para confirmar. Qualquer outra tecla cancela.\nNão há confirmação por Enter.", confirm.label), "y confirmar | qualquer outra tecla cancelar".to_string())
    } else if let Some(editor) = &app.editor {
        let target = match &editor.target { Target::Acl => "ACL".into(), Target::Config(id, node, enabled) => format!("{} / {node} / enabled={enabled}", safe_text(id)) };
        // The explicit editor is the only view showing unredacted configuration.
        // No editor buffer is ever included in status/errors or persisted.
        let row = editor.text[..editor.cursor].bytes().filter(|b| *b == b'\n').count();
        let height = areas[1].height.saturating_sub(5).max(1) as usize;
        scroll = row.saturating_sub(height.saturating_sub(1)).min(u16::MAX as usize) as u16;
        let mut text = editor.text.clone();
        text.insert(editor.cursor, '▏');
        (format!("Editar {target} — contém dados sensíveis; somente memória\n{} / {} bytes\n\n{text}", editor.text.len(), MAX_FORM), "Ctrl+S validar/salvar | Esc descartar | F2 enabled | setas/Home/End editar".into())
    } else if !app.authenticated {
        let login = &app.login;
        let marker = |n| if login.field == n { ">" } else { " " };
        (format!("{} Servidor: {}\n{} Usuário: {}\n{} Senha: {}\n{} [{}] Permitir HTTP overlay inseguro\n\nServidor obrigatório: https://host[:porta].\nHTTP envia senha/sessão sem TLS: use somente em overlay confiável e marque opt-in.\nSem descoberta automática, sem persistência de credenciais.", marker(0), safe_text(&login.server), marker(1), safe_text(&login.user), marker(2), login.masked_password(), marker(3), if login.insecure { "x" } else { " " }), "Tab/↑/↓ campo | Espaço opt-in HTTP | Enter login | Esc sair".into())
    } else {
        match app.page {
            Page::Overview => (app.overview.as_ref().map(|value| json_text(&serde_json::to_value(value).unwrap_or(Value::Null))).unwrap_or_else(|| "r para obter overview do servidor autenticado.".into()), "Tab/←/→ navegar | 1–5 página | r atualizar | PgUp/PgDn rolar | q sair".into()),
            Page::Nodes => {
                let text = if app.node_detail {
                    app.nodes.get(app.selected_node).map(|value| json_text(&serde_json::to_value(value).unwrap_or(Value::Null))).unwrap_or_else(|| "Nenhum nó.".into())
                } else {
                    // Keep the selected row visible even for large node lists.
                    scroll = app.selected_node.saturating_sub(areas[1].height.saturating_sub(4) as usize).min(u16::MAX as usize) as u16;
                    app.nodes.iter().enumerate().map(|(i, node)| format!("{} {}", if i == app.selected_node { ">" } else { " " }, node_label(node))).collect::<Vec<_>>().join("\n")
                };
                (text, "↑/↓ selecionar | Enter detalhes | a aprovar | d revogar | r atualizar | Esc voltar | q sair".into())
            }
            Page::Acl => (app.acl.as_ref().map(|value| json_text(&serde_json::to_value(value).unwrap_or(Value::Null))).unwrap_or_else(|| "r para obter política ACL.".into()), "e editar política JSON (não o envelope) | r atualizar | PgUp/PgDn rolar | q sair".into()),
            Page::Extensions => {
                let text = if let Some((id, node, value)) = &app.config {
                    format!("{} / {node}\n{}", safe_text(id), json_text(value))
                } else {
                    let node = app.nodes.get(app.selected_node).map(node_label).unwrap_or_else(|| "Nenhum nó disponível".into());
                    let rows = app.extensions.iter().enumerate().map(|(i, value)| format!("{} {} — {}", if i == app.selected_extension { ">" } else { " " }, safe_text(value["id"].as_str().unwrap_or("?")), safe_text(value["name"].as_str().unwrap_or("")))).collect::<Vec<_>>().join("\n");
                    scroll = app.selected_extension.saturating_sub(areas[1].height.saturating_sub(7) as usize).min(u16::MAX as usize) as u16;
                    format!("Nó alvo (n próximo): {node}\n\n{rows}")
                };
                (text, "↑/↓ extensão | n próximo nó | Enter config | e editar | a ativar/d desativar | R reload | Esc voltar".into())
            }
            Page::Status => (format!("Status local da sessão (não é audit log remoto).\nO SDK não oferece endpoint de logs. Nenhum segredo/corpo de erro é registrado.\n\n{}", app.status.iter().copied().collect::<Vec<_>>().join("\n")), "PgUp/PgDn rolar | Tab navegar | q sair (logout remoto best-effort)".into()),
        }
    };
    frame.render_widget(Paragraph::new(body).scroll((scroll, 0)).wrap(Wrap { trim: false })
        .block(Block::default().borders(Borders::ALL)), areas[1]);
    let status = if app.busy { "Aguardando API… Esc sair; resultado de escrita pode ser incerto." }
        else { app.status.back().copied().unwrap_or("Pronto.") };
    frame.render_widget(Paragraph::new(format!("{status}\n{help}"))
        .style(Style::default().fg(if app.busy { Color::Yellow } else { Color::Cyan })), areas[2]);
}

/// Separate terminal owner; later integration must suspend/restore the normal
/// TUI before invoking this entrypoint, never nest alternate-screen owners.
struct Screen;
impl Screen {
    fn enter() -> anyhow::Result<Self> {
        enable_raw_mode()?;
        if let Err(error) = execute!(io::stdout(), EnterAlternateScreen) {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen, Show);
            return Err(error.into());
        }
        Ok(Self)
    }
}
impl Drop for Screen {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, Show);
    }
}

/// Run the standalone remote-admin console. The optional server only pre-fills
/// the form; HTTP always requires a separate explicit opt-in on the form.
/// This function starts no local agent/server and reads/writes no state files.
pub fn run(server: Option<String>) -> anyhow::Result<()> {
    run_with_tick(server, || {})
}

/// Internal integration hook for a suspended local TUI. Called on the UI thread
/// once per event-loop round (roughly every 50 ms); the caller may drain its
/// existing message queue without drawing or owning this terminal. The callback
/// must remain nonblocking and must not enter another terminal screen.
pub(crate) fn run_with_tick(
    server: Option<String>,
    mut tick: impl FnMut(),
) -> anyhow::Result<()> {
    let (commands, rx) = mpsc::sync_channel(1);
    let (tx, replies) = mpsc::sync_channel(2);
    let worker = thread::Builder::new().name("wm-remote-admin".into()).spawn(move || worker(rx, tx))?;
    let result = (|| -> anyhow::Result<()> {
        let _screen = Screen::enter()?;
        let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
        let mut app = App::new(server);
        loop {
            tick();
            loop {
                match replies.try_recv() {
                    Ok(reply) => {
                        let logged_in = matches!(&reply, Reply::LoggedIn);
                        app.receive(reply);
                        if logged_in { app.send(&commands, Command::Fetch(Page::Overview)); }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        app.busy = false;
                        app.note("Worker encerrado; saia e tente novamente.");
                        break;
                    }
                }
            }
            terminal.draw(|frame| render(frame, &app))?;
            if event::poll(Duration::from_millis(50))? {
                if let Event::Key(key) = event::read()? {
                    if app.key(key, &commands) { break; }
                }
            }
        }
        Ok(())
    })();
    // Restore the terminal BEFORE waiting for bounded in-flight work/logout.
    // Drop the response receiver too, so a result can never deadlock shutdown.
    drop(commands);
    drop(replies);
    let _ = worker.join();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(code: KeyCode) -> KeyEvent { KeyEvent::new(code, KeyModifiers::NONE) }

    #[test]
    fn login_masks_unicode_and_http_is_opt_in() {
        let mut login = Login::new(Some("https://admin.example".into()));
        login.password = "päss🔒".into();
        assert_eq!(login.masked_password(), "*****");
        assert!(!login.insecure);
        assert!(!login.masked_password().contains("päss"));
    }
    #[test]
    fn confirmation_requires_lowercase_y_and_retains_exact_target() {
        let id = Uuid::new_v4();
        for cancel in [KeyCode::Enter, KeyCode::Esc, KeyCode::Char('Y'), KeyCode::Char('q')] {
            assert!(Confirmation { label: "revoke".into(), command: Command::Revoke(id) }.resolve(cancel).is_none());
        }
        assert!(matches!(Confirmation { label: "revoke".into(), command: Command::Revoke(id) }.resolve(KeyCode::Char('y')), Some(Command::Revoke(target)) if target == id));
    }
    #[test]
    fn redaction_is_recursive_and_case_insensitive() {
        let value = serde_json::json!({"safe": "ok", "CSRF_Token": "abc", "nodes": [{"credentials_json": "secret", "password": "hidden", "nested": {"Authorization": "Bearer xyz"}}]});
        let text = json_text(&value);
        for secret in ["abc", "secret", "hidden", "Bearer xyz"] { assert!(!text.contains(secret)); }
        assert!(text.contains("ok"));
        assert!(text.contains("[redacted]"));
    }
    #[test]
    fn editor_unicode_boundaries_and_size_limit() {
        let mut editor = Editor::new(Target::Acl, "a🔒é".into()).unwrap();
        editor.left(); editor.backspace();
        assert_eq!(editor.text, "aé");
        editor.delete();
        assert_eq!(editor.text, "a");
        assert!(Editor::new(Target::Acl, "x".repeat(MAX_FORM + 1)).is_none());
        let mut full = Editor::new(Target::Acl, "x".repeat(MAX_FORM)).unwrap();
        full.insert('é');
        assert_eq!(full.text.len(), MAX_FORM);
    }
    #[test]
    fn invalid_json_never_creates_confirmation() {
        let mut app = App::new(None);
        app.editor = Editor::new(Target::Acl, "{".into());
        app.editor_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert!(app.confirm.is_none());
        assert!(app.editor.is_some());
    }
    #[test]
    fn editor_cancel_never_sends_request() {
        let mut app = App::new(None);
        app.authenticated = true;
        app.editor = Editor::new(Target::Acl, "{}".into());
        let (tx, rx) = mpsc::sync_channel(1);
        assert!(!app.key(key(KeyCode::Esc), &tx));
        assert!(app.editor.is_none());
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn status_is_bounded_and_selection_handles_empty_lists() {
        let mut app = App::new(None);
        for _ in 0..MAX_STATUS + 10 { app.note("safe static status"); }
        assert_eq!(app.status.len(), MAX_STATUS);
        app.selected_node = 100;
        app.selected_extension = 100;
        app.clamp_selection();
        assert_eq!(app.selected_node, 0);
        assert_eq!(app.selected_extension, 0);
        assert!(app.node_id().is_none());
    }
    #[test]
    fn login_hands_password_to_worker_without_retaining_it_in_form() {
        let mut app = App::new(Some("https://admin.example".into()));
        app.login.password = "unique-password".into();
        let (tx, rx) = mpsc::sync_channel(1);
        assert!(!app.key(key(KeyCode::Enter), &tx));
        assert!(app.login.password.is_empty());
        assert!(app.busy);
        match rx.try_recv().unwrap() {
            Command::Login { server, user, password, insecure } => {
                assert_eq!(server, "https://admin.example");
                assert_eq!(user, "admin");
                assert_eq!(password, "unique-password");
                assert!(!insecure);
            }
            _ => panic!("expected login command"),
        }
        assert!(app.status.iter().all(|message| !message.contains("unique-password")));
    }
    #[test]
    fn worker_queue_has_single_flight_semantics() {
        let mut app = App::new(None);
        let (tx, rx) = mpsc::sync_channel(1);
        app.send(&tx, Command::Fetch(Page::Overview));
        app.send(&tx, Command::Revoke(Uuid::new_v4()));
        assert!(matches!(rx.try_recv(), Ok(Command::Fetch(Page::Overview))));
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn config_form_confirmation_captures_exact_node_and_enabled_state() {
        let mut app = App::new(None);
        let node = Uuid::new_v4();
        app.editor = Editor::new(Target::Config("exec-demo".into(), node, false), "{\"port\": 1234}".into());
        app.editor_key(key(KeyCode::F(2)));
        app.editor_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert!(app.editor.is_none());
        let confirmation = app.confirm.take().unwrap();
        assert!(matches!(confirmation.resolve(KeyCode::Char('y')), Some(Command::PutConfig(id, target, true, value)) if id == "exec-demo" && target == node && value["port"] == 1234));
    }
    #[test]
    fn render_never_exposes_login_password() {
        let mut app = App::new(Some("https://admin.example".into()));
        app.login.password = "never-render-this-password".into();
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let screen = terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect::<String>();
        assert!(!screen.contains("never-render-this-password"));
        assert!(screen.contains("**************************"));
    }
    #[test]
    fn control_sequences_are_not_rendered_as_terminal_commands() {
        assert_eq!(safe_text("node\x1b[31m\n\u{7}"), "node [31m  ");
    }
}
