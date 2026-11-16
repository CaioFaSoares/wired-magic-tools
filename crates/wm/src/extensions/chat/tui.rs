//! TUI chat client (`wm chat`).
//!
//! The default chat experience is a terminal client. It talks to a running
//! `chat_gui` sidecar over the same HTTP API the web UI uses, so both clients
//! share one port, one message store and one mesh exposure. The server is
//! usually on another node; point `--url` at `http://<overlay_ip>:8790` or at
//! a local forwarded port.

use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use chrono::Local;
use super::core::{ChatMessage as Message, MAX_MESSAGES};
use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use serde::Deserialize;

const POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Deserialize)]
struct Info {
    title: String,
    #[serde(default)]
    node: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MessagesResponse {
    #[serde(default)]
    epoch: Option<String>,
    #[serde(default)]
    messages: Vec<Message>,
}

#[derive(Debug, Deserialize)]
struct PostResponse {
    #[serde(default)]
    epoch: Option<String>,
    message: Message,
}

pub fn run(url: String, author: Option<String>) -> anyhow::Result<()> {
    let url = url.trim_end_matches('/').to_string();
    if !url.starts_with("http://") && !url.starts_with("https://") {
        bail!("--url precisa começar com http:// ou https://");
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .context("não foi possível criar o cliente HTTP")?;

    let info = client
        .get(format!("{url}/api/info"))
        .send()
        .with_context(|| format!("não foi possível alcançar {url}; o chat_gui está rodando?"))?
        .error_for_status()
        .context("o chat_gui recusou /api/info")?
        .json::<Info>()
        .context("resposta inválida de /api/info")?;

    let mut state = State {
        url,
        author: author.unwrap_or_else(|| "anônimo".into()),
        title: info.title,
        node: info.node,
        messages: Vec::new(),
        seen: std::collections::HashSet::new(),
        cursor: 0,
        epoch: None,
        input: String::new(),
        status: "conectando…".into(),
        online: true,
        scroll: 0,
        last_poll: Instant::now() - POLL_INTERVAL,
    };
    state.poll(&client);

    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &client, &mut state);
    ratatui::restore();
    result
}

struct State {
    url: String,
    author: String,
    title: String,
    node: Option<String>,
    messages: Vec<Message>,
    seen: std::collections::HashSet<u64>,
    cursor: u64,
    epoch: Option<String>,
    input: String,
    status: String,
    online: bool,
    scroll: u16,
    last_poll: Instant,
}

impl State {
    fn poll(&mut self, client: &reqwest::blocking::Client) {
        let result = (|| -> anyhow::Result<Vec<Message>> {
            let response = client
                .get(format!("{}/api/messages?after={}", self.url, self.cursor))
                .send()?
                .error_for_status()?;
            let body: MessagesResponse = response.json()?;
            if let Some(epoch) = body.epoch {
                if self.epoch.as_deref().is_some_and(|current| current != epoch) {
                    self.messages.clear();
                    self.seen.clear();
                    self.cursor = 0;
                    self.epoch = Some(epoch.clone());
                    return Ok(Vec::new()); // next poll reloads from zero
                }
                self.epoch = Some(epoch);
            }
            Ok(body.messages)
        })();

        match result {
            Ok(messages) => {
                self.merge(messages);
                self.online = true;
                self.status = format!("sala conectada · {} mensagem(ns)", self.messages.len());
            }
            Err(err) => {
                self.online = false;
                self.status = format!("sem conexão: {err}");
            }
        }
    }

    fn merge(&mut self, messages: Vec<Message>) {
        let before = self.messages.len();
        for message in messages {
            if self.seen.insert(message.id) {
                self.cursor = self.cursor.max(message.id);
                self.messages.push(message);
            }
        }
        if self.messages.len() > MAX_MESSAGES {
            let drop = self.messages.len() - MAX_MESSAGES;
            self.messages.drain(0..drop);
        }
        if self.messages.len() > before {
            self.scroll = u16::MAX;
        }
    }

    fn send(&mut self, client: &reqwest::blocking::Client) {
        let text = self.input.trim().to_string();
        if text.is_empty() {
            return;
        }
        let result = client
            .post(format!("{}/api/messages", self.url))
            .json(&serde_json::json!({ "author": self.author, "text": text }))
            .send()
            .and_then(|response| response.error_for_status())
            .and_then(|response| response.json::<PostResponse>());
        match result {
            Ok(response) => {
                self.input.clear();
                if let Some(epoch) = response.epoch {
                    self.epoch = Some(epoch);
                }
                self.merge(vec![response.message]);
                self.online = true;
                self.status = "mensagem enviada".into();
            }
            Err(err) => {
                self.online = false;
                self.status = format!("falha ao enviar: {err}");
            }
        }
    }
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    client: &reqwest::blocking::Client,
    state: &mut State,
) -> anyhow::Result<()> {
    loop {
        if state.last_poll.elapsed() >= POLL_INTERVAL {
            state.poll(client);
            state.last_poll = Instant::now();
        }
        terminal.draw(|frame| draw(frame, state))?;

        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Esc => break,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
            KeyCode::Enter => state.send(client),
            KeyCode::Backspace => {
                state.input.pop();
            }
            KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                state.input.push(ch);
            }
            KeyCode::PageUp => state.scroll = state.scroll.saturating_sub(8),
            KeyCode::PageDown => {
                state.scroll = state.scroll.saturating_add(8);
            }
            _ => {}
        }
    }
    Ok(())
}

fn draw(frame: &mut Frame, state: &mut State) {
    let area = frame.area();
    let [header, messages, composer, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(3),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(frame, header, state);
    draw_messages(frame, messages, state);
    draw_composer(frame, composer, state);
    draw_footer(frame, footer, state);
}

fn draw_header(frame: &mut Frame, area: Rect, state: &State) {
    let node = state.node.as_deref().unwrap_or("—");
    let dot = if state.online { "●" } else { "○" };
    let color = if state.online { Color::Green } else { Color::Red };
    let title = format!(" {}  ·  nó {} ", state.title, node);
    let status = format!("{dot} {}", state.status);
    let paragraph = Paragraph::new(vec![
        Line::from(Span::styled(
            title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(status, Style::default().fg(color))),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    frame.render_widget(paragraph, area);
}

fn draw_messages(frame: &mut Frame, area: Rect, state: &mut State) {
    let mut lines: Vec<Line> = Vec::new();
    if state.messages.is_empty() {
        lines.push(Line::from(Span::styled(
            "A sala está vazia. Escreva a primeira mensagem.",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for message in &state.messages {
            let time = message
                .at
                .with_timezone(&Local)
                .format("%H:%M")
                .to_string();
            lines.push(Line::from(vec![
                Span::styled(
                    message.author.clone(),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(time, Style::default().fg(Color::DarkGray)),
            ]));
            for text_line in message.text.lines() {
                lines.push(Line::from(format!("  {text_line}")));
            }
            lines.push(Line::from(""));
        }
    }

    let inner_height = area.height.saturating_sub(2) as usize;
    let max_scroll = lines.len().saturating_sub(inner_height) as u16;
    if state.scroll == u16::MAX || state.scroll > max_scroll {
        state.scroll = max_scroll;
    }

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" mensagens ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .wrap(Wrap { trim: false })
        .scroll((state.scroll, 0));
    frame.render_widget(paragraph, area);
}

fn draw_composer(frame: &mut Frame, area: Rect, state: &State) {
    let line = Line::from(vec![
        Span::styled(
            format!("{}: ", state.author),
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(state.input.clone()),
        Span::styled("▌", Style::default().fg(Color::Cyan)),
    ]);
    let paragraph = Paragraph::new(line).block(
        Block::default()
            .title(" mensagem ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
    );
    frame.render_widget(paragraph, area);
}

fn draw_footer(frame: &mut Frame, area: Rect, state: &State) {
    let hint = if state.input.is_empty() {
        "Enter envia · PgUp/PgDn rola · Esc/Ctrl+C sai"
    } else {
        "Enter envia · Backspace apaga · Esc sai sem enviar"
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            hint,
            Style::default().fg(Color::DarkGray),
        ))),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn tui_draw_does_not_panic() {
        let mut state = State {
            url: "http://127.0.0.1:1".into(),
            author: "teste".into(),
            title: "Chat".into(),
            node: Some("nó".into()),
            messages: vec![Message {
                id: 1,
                author: "Ana".into(),
                text: "olá\nmundo".into(),
                at: chrono::Utc::now(),
            }],
            seen: std::collections::HashSet::from([1]),
            cursor: 1,
            epoch: None,
            input: "rascunho".into(),
            status: "teste".into(),
            online: true,
            scroll: u16::MAX,
            last_poll: Instant::now(),
        };
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &mut state)).unwrap();
    }
}
