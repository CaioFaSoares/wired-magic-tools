//! Rendering for the wired-magic TUI.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Wrap};

use wm_proto::{PathKind, ShareKind};

use crate::actions;
use crate::app::{
    App, FieldKind, Level, MouseAction, TABS, TAB_AGENT, TAB_CONSOLE, TAB_HELP, TAB_PEERS,
    TAB_SERVICES, TAB_SHARES, TAB_STATUS, TAB_TOPOLOGY,
};

const BORDER: Color = Color::DarkGray;
const HIGHLIGHT: Color = Color::Rgb(22, 40, 52);

pub fn draw(frame: &mut Frame, app: &mut App) {
    app.clear_hits();
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(2),
            Constraint::Min(5),
            Constraint::Length(1),
            Constraint::Length(2),
        ])
        .split(area);
    app.tabs_rect = chunks[1];
    app.body_rect = chunks[2];
    app.actions_rect = chunks[3];

    draw_header(frame, chunks[0], app);
    draw_tabs(frame, chunks[1], app);
    match app.tab {
        TAB_STATUS => draw_status(frame, chunks[2], app),
        TAB_PEERS => draw_peers(frame, chunks[2], app),
        TAB_SHARES => draw_shares(frame, chunks[2], app),
        TAB_TOPOLOGY => draw_topology(frame, chunks[2], app),
        TAB_AGENT => draw_agent(frame, chunks[2], app),
        TAB_SERVICES => draw_services(frame, chunks[2], app),
        TAB_CONSOLE => draw_console(frame, chunks[2], app),
        TAB_HELP => draw_help(frame, chunks[2], app),
        _ => {}
    }
    draw_action_bar(frame, chunks[3], app);
    draw_footer(frame, chunks[4], app);

    if app.menu.is_some() {
        draw_menu(frame, area, app);
    } else if app.form.is_some() {
        draw_form(frame, area, app);
    } else if app.confirm.is_some() {
        draw_confirm(frame, area, app);
    }
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let name = app
        .status
        .as_ref()
        .and_then(|status| status.node_name.clone())
        .or_else(|| app.node.as_ref().map(|node| node.node_name.clone()))
        .unwrap_or_else(|| "wired-magic".into());
    let ip = app
        .status
        .as_ref()
        .and_then(|status| status.overlay_ip)
        .map(|ip| ip.to_string())
        .or_else(|| app.node.as_ref().map(|node| node.overlay_ip.clone()))
        .unwrap_or_else(|| "sem inscrição".into());
    let control = app
        .live
        .then(|| app.status.as_ref().is_some_and(|status| status.control_connected));
    let relay = app
        .live
        .then(|| app.status.as_ref().is_some_and(|status| status.relay_connected));

    let control_span = match control {
        Some(true) => Span::styled("● control", Style::default().fg(Color::Green)),
        Some(false) => Span::styled("○ control", Style::default().fg(Color::Red)),
        None => Span::styled("○ control", Style::default().fg(Color::DarkGray)),
    };
    let relay_span = match relay {
        Some(true) => Span::styled("● relay", Style::default().fg(Color::Green)),
        Some(false) => Span::styled("○ relay", Style::default().fg(Color::DarkGray)),
        None => Span::styled("○ relay", Style::default().fg(Color::DarkGray)),
    };
    let agent_style = if app.agent.stopping.is_some() || app.agent.starting.is_some() {
        Style::default().fg(Color::Cyan)
    } else if app.live {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::Yellow)
    };
    let last = app
        .updated
        .map(|t| format!("{}s", t.elapsed().as_secs()))
        .unwrap_or_else(|| "—".into());
    let line = Line::from(vec![
        Span::styled(
            " wired-magic ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(name, Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("  "),
        Span::styled(ip, Style::default().fg(Color::Yellow)),
        Span::raw("   "),
        control_span,
        Span::raw("  "),
        relay_span,
        Span::raw("   agente "),
        Span::styled(app.agent_label(), agent_style),
    ]);

    let control_url = app
        .status
        .as_ref()
        .and_then(|status| status.control_url.clone())
        .or_else(|| app.node.as_ref().map(|node| node.control_url.clone()))
        .unwrap_or_else(|| "—".into());
    let session = app
        .session_started
        .with_timezone(&chrono::Local)
        .format("%d/%m %H:%M");
    let url_limit = (area.width as usize).saturating_sub(58).max(12);
    let second = Line::from(vec![
        Span::styled(" inscrição ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            if app.is_enrolled() { "salva" } else { "pendente" },
            Style::default().fg(if app.is_enrolled() {
                Color::Green
            } else {
                Color::Yellow
            }),
        ),
        Span::styled(" · control ", Style::default().fg(Color::DarkGray)),
        Span::raw(truncate(&control_url, url_limit)),
        Span::styled(
            format!(" · há {last} · sessão {session}"),
            Style::default().fg(Color::DarkGray),
        ),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(BORDER));
    frame.render_widget(Paragraph::new(vec![line, second]).block(block), area);
}

fn draw_tabs(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let mut spans: Vec<Span> = Vec::new();
    let mut x = area.x;
    for (index, title) in TABS.iter().enumerate() {
        let label = format!(" {title} ");
        let width = label.chars().count() as u16;
        if x.saturating_add(width) > area.right() {
            break;
        }
        let rect = Rect {
            x,
            y: area.y,
            width,
            height: area.height,
        };
        let selected = app.tab == index;
        let hovered = !app.modal_open() && app.mouse_inside(rect);
        let style = if selected {
            Style::default()
                .fg(Color::Cyan)
                .bg(Color::Rgb(18, 32, 40))
                .add_modifier(Modifier::BOLD)
        } else if hovered {
            Style::default().fg(Color::Black).bg(Color::Cyan)
        } else {
            Style::default().fg(Color::Gray)
        };
        spans.push(Span::styled(label, style));
        app.register_hit(rect, MouseAction::Tab(index));
        x = x.saturating_add(width);
        if index + 1 < TABS.len() && x < area.right() {
            spans.push(Span::styled("·", Style::default().fg(Color::DarkGray)));
            app.register_hit(
                Rect {
                    x,
                    y: area.y,
                    width: 1,
                    height: area.height,
                },
                MouseAction::Tab(index),
            );
            x = x.saturating_add(1);
        }
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 1,
        },
    );
}

#[derive(Clone)]
struct ButtonSpec {
    label: &'static str,
    action: MouseAction,
}

fn button(label: &'static str, action: MouseAction) -> ButtonSpec {
    ButtonSpec { label, action }
}

fn action_bar_specs(app: &App) -> Vec<ButtonSpec> {
    let mut specs = Vec::new();
    match app.tab {
        TAB_STATUS => {
            if !app.is_enrolled() {
                specs.push(button("Entrar", MouseAction::Join));
                specs.push(button("Comando", MouseAction::OpenConsole));
            } else {
                if app.live {
                    specs.push(button("Parar", MouseAction::StopAgent));
                    specs.push(button("Reiniciar", MouseAction::RestartAgent));
                } else {
                    specs.push(button("Iniciar", MouseAction::StartAgent));
                }
                specs.push(button("Auto", MouseAction::ToggleAutoStart));
                specs.push(button("Ajustes", MouseAction::AgentSettings));
                specs.push(button("Log", MouseAction::AgentLog));
                specs.push(button("GUI", MouseAction::OpenGui));
                specs.push(button("Painel", MouseAction::AdminPanel));
            }
        }
        TAB_PEERS => {
            specs.push(button("Ping", MouseAction::Ping));
            specs.push(button("Probe", MouseAction::Probe));
            specs.push(button("Reconectar", MouseAction::Reconnect));
            specs.push(button("Copiar", MouseAction::CopyPeer));
            specs.push(button("Renomear", MouseAction::RenameNode));
        }
        TAB_SHARES => {
            specs.push(button("Novo", MouseAction::NewShare));
            specs.push(button("Abrir", MouseAction::OpenShare));
            specs.push(button("Copiar", MouseAction::CopyShare));
            specs.push(button("Renomear", MouseAction::RenameShare));
            specs.push(button("On/Off", MouseAction::ToggleShare));
            specs.push(button("Remover", MouseAction::DeleteShare));
        }
        TAB_TOPOLOGY => {
            specs.push(button("Peers", MouseAction::Tab(TAB_PEERS)));
            specs.push(button("Shares", MouseAction::Tab(TAB_SHARES)));
            specs.push(button("Agente", MouseAction::Tab(TAB_AGENT)));
            specs.push(button("Console", MouseAction::Tab(TAB_CONSOLE)));
        }
        TAB_AGENT => {
            if app.live {
                specs.push(button("Parar", MouseAction::StopAgent));
                specs.push(button("Reiniciar", MouseAction::RestartAgent));
            } else {
                specs.push(button("Iniciar", MouseAction::StartAgent));
            }
            specs.push(button("Auto", MouseAction::ToggleAutoStart));
            specs.push(button("Ajustes", MouseAction::AgentSettings));
            specs.push(button("Log", MouseAction::AgentLog));
            specs.push(button("Serviço", MouseAction::InstallService));
        }
        TAB_SERVICES => {
            let gui_active = app
                .jobs
                .iter()
                .any(|job| job.label == "gui" && crate::agent::pid_alive(job.pid));
            if gui_active {
                specs.push(button("Fechar GUI", MouseAction::CloseGui));
            } else {
                specs.push(button("Abrir GUI", MouseAction::OpenGui));
            }
            specs.push(button("Painel", MouseAction::AdminPanel));
            specs.push(button("Log", MouseAction::JobLog));
            specs.push(button("Parar job", MouseAction::StopSelectedJob));
            specs.push(button("Instalar", MouseAction::InstallService));
            specs.push(button("Status", MouseAction::ServiceStatus));
            specs.push(button("Doctor", MouseAction::Doctor));
        }
        TAB_CONSOLE => {
            specs.push(button("Comando", MouseAction::OpenConsole));
            specs.push(button("Limpar", MouseAction::ClearConsole));
        }
        TAB_HELP => {
            specs.push(button("Início", MouseAction::Tab(TAB_STATUS)));
            specs.push(button("Peers", MouseAction::Tab(TAB_PEERS)));
            specs.push(button("Shares", MouseAction::Tab(TAB_SHARES)));
            specs.push(button("Agente", MouseAction::Tab(TAB_AGENT)));
            specs.push(button("Console", MouseAction::Tab(TAB_CONSOLE)));
        }
        _ => {}
    }
    specs
}

fn draw_action_bar(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let menu_width = 7.min(area.width);
    let menu_rect = Rect {
        x: area.right().saturating_sub(menu_width),
        y: area.y,
        width: menu_width,
        height: 1,
    };
    let limit = menu_rect.x;
    let mut x = area.x;
    for spec in action_bar_specs(app) {
        let label = format!(" {} ", spec.label);
        let width = label.chars().count() as u16;
        if x.saturating_add(width) > limit {
            break;
        }
        let rect = Rect {
            x,
            y: area.y,
            width,
            height: 1,
        };
        draw_button(frame, app, rect, &label, spec.action, false);
        x = x.saturating_add(width + 1);
    }
    draw_button(frame, app, menu_rect, " Menu ", MouseAction::OpenMenu, false);
}

fn draw_button(
    frame: &mut Frame,
    app: &mut App,
    rect: Rect,
    label: &str,
    action: MouseAction,
    overlay: bool,
) {
    let hovered = app.mouse_inside(rect) && (overlay || !app.modal_open());
    let style = if hovered {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::White)
            .bg(Color::Rgb(30, 45, 55))
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(label.to_string(), style)))
            .alignment(Alignment::Center),
        rect,
    );
    if overlay {
        app.register_overlay_hit(rect, action);
    } else {
        app.register_hit(rect, action);
    }
}

fn draw_button_row(frame: &mut Frame, area: Rect, app: &mut App, specs: &[ButtonSpec]) {
    let mut x = area.x;
    for spec in specs {
        let label = format!(" {} ", spec.label);
        let width = label.chars().count() as u16;
        if x.saturating_add(width) > area.right() {
            break;
        }
        let rect = Rect {
            x,
            y: area.y,
            width,
            height: 1,
        };
        draw_button(frame, app, rect, &label, spec.action.clone(), true);
        x = x.saturating_add(width + 1);
    }
}

fn register_table_rows(
    app: &mut App,
    area: Rect,
    offset: usize,
    len: usize,
    action: impl Fn(usize) -> MouseAction,
) {
    let visible = area.height.saturating_sub(3) as usize;
    if visible == 0 || area.width < 3 {
        return;
    }
    let bottom = area
        .y
        .saturating_add(area.height)
        .saturating_sub(1);
    for row in 0..visible {
        let index = offset + row;
        if index >= len {
            break;
        }
        let y = area.y.saturating_add(2 + row as u16);
        if y >= bottom {
            break;
        }
        app.register_hit(
            Rect {
                x: area.x + 1,
                y,
                width: area.width.saturating_sub(2),
                height: 1,
            },
            action(index),
        );
    }
}

fn block(title: impl Into<String>) -> Block<'static> {
    Block::default()
        .title(title.into())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(BORDER))
}

fn kv(label: &'static str, value: impl Into<String>) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<14}"), Style::default().fg(Color::DarkGray)),
        Span::raw(value.into()),
    ])
}

fn ok_off(value: bool) -> Span<'static> {
    if value {
        Span::styled("ok", Style::default().fg(Color::Green))
    } else {
        Span::styled("off", Style::default().fg(Color::Red))
    }
}

fn truncate(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn fit_tail(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let skip = count - max.saturating_sub(1);
    let mut out = String::from("…");
    out.extend(text.chars().skip(skip));
    out
}

fn stale(app: &App, title: &str) -> String {
    if !app.live && app.status.is_some() {
        format!(" {title} · última sessão ")
    } else {
        format!(" {title} ")
    }
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    let mut lines: Vec<Line> = Vec::new();
    if !app.live && app.is_enrolled() {
        lines.push(Line::from(Span::styled(
            if app.agent.starting.is_some() {
                "agente iniciando em segundo plano… os dados abaixo são da última sessão"
            } else if app.agent.stopping.is_some() {
                "agente parando… os dados abaixo são da última sessão"
            } else {
                "agente parado — os dados abaixo são da última sessão. Pressione s para iniciar."
            },
            Style::default().fg(Color::Yellow),
        )));
        lines.push(Line::from(""));
    }
    if let Some(err) = &app.error {
        lines.push(Line::from(Span::styled(
            format!("agente: {err}"),
            Style::default().fg(Color::Red),
        )));
        lines.push(Line::from(""));
    }

    if !app.is_enrolled() {
        if let Some(err) = &app.node_error {
            lines.push(Line::from(Span::styled(
                format!("inscrição ilegível: {err}"),
                Style::default().fg(Color::Red),
            )));
            lines.push(Line::from(Span::styled(
                "se o nó já está inscrito, reabra com sudo wm tui",
                Style::default().fg(Color::Yellow),
            )));
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            "Esta máquina ainda não entrou na malha.",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from("  1. peça um convite ao administrador (código wmc_…)"));
        lines.push(Line::from("  2. pressione j e cole o código, ou informe URL + token"));
        lines.push(Line::from("  3. a TUI inscreve, inicia o agente e mantém a conexão"));
        lines.push(Line::from("     ativa mesmo depois de sair com q"));
        lines.push(Line::from(""));
        if !actions::is_root() {
            lines.push(Line::from(Span::styled(
                "sem root: iniciar o agente exige root (use sudo wm tui)",
                Style::default().fg(Color::Yellow),
            )));
        }
        lines.push(Line::from(Span::styled(
            "atalhos: j entrar · :comando · ? ajuda · q sair (nada é iniciado)",
            Style::default().fg(Color::DarkGray),
        )));
    } else if let Some(status) = &app.status {
        let path = |ok: bool| ok_off(ok);
        lines.push(kv(
            "nó",
            format!(
                "{} ({})",
                status.node_name.clone().unwrap_or_default(),
                status
                    .node_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "—".into())
            ),
        ));
        lines.push(kv(
            "overlay",
            status
                .overlay_ip
                .map(|ip| ip.to_string())
                .unwrap_or_else(|| "—".into()),
        ));
        lines.push(kv(
            "control",
            format!(
                "{}   {}",
                status.control_url.clone().unwrap_or_default(),
                if status.control_connected {
                    "conectado"
                } else {
                    "desconectado"
                }
            ),
        ));
        lines.push(kv(
            "relay",
            if status.relay_connected {
                "conectado"
            } else {
                "desconectado".into()
            },
        ));
        lines.push(kv(
            "tun / udp",
            format!(
                "{}  :{}",
                status.tun_name.clone().unwrap_or_else(|| "—".into()),
                status
                    .udp_port
                    .map(|port| port.to_string())
                    .unwrap_or_else(|| "—".into())
            ),
        ));
        lines.push(kv("nat", format!("{:?}", status.nat_type).to_lowercase()));
        lines.push(kv(
            "peers",
            format!("{}/{} online", status.peers_online, status.peers_total),
        ));
        lines.push(kv("mapa", format!("v{}", status.map_version)));
        lines.push(kv("uptime", format!("{}s", status.uptime_secs)));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::raw("control "),
            path(status.control_connected),
            Span::raw("   relay "),
            path(status.relay_connected),
            Span::raw("   versão "),
            Span::raw(status.version.clone()),
        ]));
        if let Some(err) = &status.last_error {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("último erro: {err}"),
                Style::default().fg(Color::Yellow),
            )));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("auto-start ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                if app.auto_start { "ligado" } else { "desligado" },
                Style::default().fg(if app.auto_start {
                    Color::Green
                } else {
                    Color::Yellow
                }),
            ),
            Span::styled(
                if app.auto_start {
                    "  (reabrir a TUI reconecta)"
                } else {
                    "  (o agente fica parado ao reabrir)"
                },
                Style::default().fg(Color::DarkGray),
            ),
        ]));
        if app.cached_at.is_some() && !app.live {
            lines.push(Line::from(Span::styled(
                format!(
                    "última sessão: {} · os dados desta tela são um retrato salvo",
                    app.cached_at
                        .map(|at| at.with_timezone(&chrono::Local).format("%d/%m %H:%M:%S").to_string())
                        .unwrap_or_else(|| "—".into())
                ),
                Style::default().fg(Color::DarkGray),
            )));
        }
    } else if let Some(node) = &app.node {
        lines.push(kv("nó", format!("{} ({})", node.node_name, node.overlay_ip)));
        lines.push(kv("control", node.control_url.clone()));
        lines.push(kv("rede", node.network_name.clone()));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "agente ainda sem resposta; pressione s para iniciar",
            Style::default().fg(Color::Yellow),
        )));
    } else {
        lines.push(Line::from(
            "sem dados do agente. Ele está rodando? `wm up` inicia o daemon.",
        ));
    }

    if app.is_enrolled() {
        lines.push(Line::from(""));
        if !actions::is_root() {
            lines.push(Line::from(Span::styled(
                "sem root: visualizar funciona; iniciar/parar exige sudo wm tui",
                Style::default().fg(Color::Yellow),
            )));
        }
        lines.push(Line::from(Span::styled(
            "s iniciar · X parar · R reiniciar · e ajustes · a auto-start · g GUI · d doctor · ? ajuda",
            Style::default().fg(Color::DarkGray),
        )));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block(" Estado ")), area);
}

fn draw_peers(frame: &mut Frame, area: Rect, app: &mut App) {
    let header = Row::new(vec![
        "Nó", "Overlay", "Caminho", "Endpoint", "RTT", "Handshake", "RX", "TX",
    ])
    .style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );
    let rows: Vec<Row> = app
        .peers
        .iter()
        .map(|peer| {
            let path_style = match peer.path {
                PathKind::Direct => Style::default().fg(Color::Green),
                PathKind::Relay => Style::default().fg(Color::Yellow),
                PathKind::None => Style::default().fg(Color::DarkGray),
            };
            Row::new(vec![
                Cell::from(peer.name.clone()),
                Cell::from(peer.overlay_ip.to_string()),
                Cell::from(peer.path.to_string()).style(path_style),
                Cell::from(peer.endpoint.clone().unwrap_or_else(|| "—".into())),
                Cell::from(
                    peer.rtt_ms
                        .map(|value| format!("{value} ms"))
                        .unwrap_or_else(|| "—".into()),
                ),
                Cell::from(
                    peer.last_handshake_secs
                        .map(|value| format!("{value}s"))
                        .unwrap_or_else(|| "—".into()),
                ),
                Cell::from(fmt_bytes(peer.rx_bytes)),
                Cell::from(fmt_bytes(peer.tx_bytes)),
            ])
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Length(16),
            Constraint::Length(13),
            Constraint::Length(8),
            Constraint::Min(18),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Length(9),
            Constraint::Length(9),
        ],
    )
    .header(header)
    .row_highlight_style(Style::default().bg(HIGHLIGHT).add_modifier(Modifier::BOLD))
    .highlight_symbol("▸ ")
    .block(block(stale(app, &format!("Peers ({})", app.peers.len()))));
    frame.render_stateful_widget(table, area, &mut app.table);
    let offset = app.table.offset();
    let peers = app.peers.len();
    register_table_rows(app, area, offset, peers, MouseAction::SelectPeer);

    if app.peers.is_empty() {
        let msg = if app.is_enrolled() {
            if app.live {
                "nenhum peer no mapa ainda"
            } else {
                "nenhum peer conhecido (agente parado)"
            }
        } else {
            "entre na malha (j) para ver peers"
        };
        empty_message(frame, area, msg);
    }
}

fn draw_shares(frame: &mut Frame, area: Rect, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(3)])
        .split(area);

    let header = Row::new(vec!["Share", "Dono", "Tipo", "Transporte", "Endereço", "Estado"]).style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );
    let rows: Vec<Row> = app
        .shares
        .iter()
        .map(|share| {
            let own = app.is_own_share(share);
            let kind_style = match share.kind {
                ShareKind::Web => Style::default().fg(Color::LightBlue),
                ShareKind::Game => Style::default().fg(Color::LightGreen),
                ShareKind::Udp => Style::default().fg(Color::Yellow),
                ShareKind::Tcp => Style::default().fg(Color::Gray),
            };
            let state = if !share.enabled {
                Span::styled("desligado", Style::default().fg(Color::DarkGray))
            } else if own {
                Span::styled("seu · ativo", Style::default().fg(Color::Green))
            } else if share.online {
                Span::styled("ativo", Style::default().fg(Color::Green))
            } else {
                Span::styled("peer offline", Style::default().fg(Color::Yellow))
            };
            Row::new(vec![
                Cell::from(format!(
                    "{}{}",
                    if own { "★ " } else { "  " },
                    share.name
                )),
                Cell::from(share.node_name.clone()),
                Cell::from(share.kind.as_str()).style(kind_style),
                Cell::from(share.protocol.as_str()),
                Cell::from(share.address()),
                Cell::from(state),
            ])
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Length(20),
            Constraint::Length(14),
            Constraint::Length(7),
            Constraint::Length(10),
            Constraint::Min(18),
            Constraint::Length(14),
        ],
    )
    .header(header)
    .row_highlight_style(Style::default().bg(HIGHLIGHT).add_modifier(Modifier::BOLD))
    .highlight_symbol("▸ ")
    .block(block(stale(app, &format!("Shares ({})", app.shares.len()))));
    frame.render_stateful_widget(table, chunks[0], &mut app.shares_table);
    let offset = app.shares_table.offset();
    let shares = app.shares.len();
    register_table_rows(app, chunks[0], offset, shares, MouseAction::SelectShare);

    if app.shares.is_empty() {
        let msg = if app.is_enrolled() && app.live {
            "nenhum serviço publicado — pressione n para publicar uma porta"
        } else if app.is_enrolled() {
            "sem conexão com o agente; shares da última sessão aparecem aqui"
        } else {
            "entre na malha (j) para ver e publicar shares"
        };
        empty_message(frame, chunks[0], msg);
    }

    let detail = if let Some(share) = app.selected_share() {
        let url = share
            .public_url
            .clone()
            .or_else(|| share.open_url())
            .unwrap_or_else(|| "—".into());
        vec![
            Line::from(vec![
                Span::styled(
                    format!("{} ", share.name),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(share.wm_url(), Style::default().fg(Color::Gray)),
                Span::styled("  ·  ", Style::default().fg(Color::DarkGray)),
                Span::raw(if share.description.trim().is_empty() {
                    "sem descrição".into()
                } else {
                    truncate(share.description.trim(), 80)
                }),
                Span::styled("  ·  ", Style::default().fg(Color::DarkGray)),
                Span::raw(truncate(&url, 60)),
            ]),
            Line::from(Span::styled(
                "Enter abrir · c copiar · n novo · r renomear · e ligar/desligar · d remover",
                Style::default().fg(Color::DarkGray),
            )),
        ]
    } else {
        vec![Line::from(Span::styled(
            "n publica uma porta local na malha",
            Style::default().fg(Color::DarkGray),
        ))]
    };
    frame.render_widget(
        Paragraph::new(detail).block(block(" Share selecionado ")),
        chunks[1],
    );
}

fn draw_topology(frame: &mut Frame, area: Rect, app: &App) {
    let mut lines = Vec::new();
    let direct = app
        .peers
        .iter()
        .filter(|peer| peer.path == PathKind::Direct)
        .count();
    let relay = app
        .peers
        .iter()
        .filter(|peer| peer.path == PathKind::Relay)
        .count();
    let idle = app.peers.len() - direct - relay;
    lines.push(Line::from(vec![
        Span::styled("  você ", Style::default().fg(Color::Cyan)),
        Span::raw(format!(
            "({})",
            app.status
                .as_ref()
                .and_then(|status| status.overlay_ip)
                .map(|ip| ip.to_string())
                .or_else(|| app.node.as_ref().map(|node| node.overlay_ip.clone()))
                .unwrap_or_else(|| "?".into())
        )),
    ]));
    lines.push(Line::from(""));
    for peer in &app.peers {
        let (icon, detail, style) = match peer.path {
            PathKind::Direct => (
                "──",
                peer.endpoint.clone().unwrap_or_else(|| "direto".into()),
                Style::default().fg(Color::Green),
            ),
            PathKind::Relay => (
                "~~",
                format!("relay {}", peer.endpoint.clone().unwrap_or_default()),
                Style::default().fg(Color::Yellow),
            ),
            PathKind::None => (
                "··",
                "sem caminho ainda".into(),
                Style::default().fg(Color::DarkGray),
            ),
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {icon} "), style),
            Span::styled(
                format!("{:<16}", peer.name),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(peer.overlay_ip.to_string(), Style::default().fg(Color::Gray)),
            Span::raw("   "),
            Span::styled(detail, style),
            Span::raw("   "),
            Span::styled(
                peer.rtt_ms
                    .map(|value| format!("{value} ms"))
                    .unwrap_or_else(|| "—".into()),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(format!(
        "  {direct} diretos · {relay} via relay · {idle} sem caminho"
    )));
    lines.push(Line::from(Span::styled(
        "  ── direto    ~~ relay    ·· sem caminho",
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(block(stale(app, "Topologia"))),
        area,
    );
}

fn draw_agent(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(11), Constraint::Min(5)])
        .split(area);

    let state = if app.agent.stopping.is_some() {
        Span::styled("parando…", Style::default().fg(Color::Cyan))
    } else if app.agent.starting.is_some() {
        Span::styled(
            format!(
                "iniciando há {}s (pid {})",
                app.agent.starting.map(|started| started.elapsed().as_secs()).unwrap_or(0),
                app.agent.pid.map(|pid| pid.to_string()).unwrap_or_else(|| "?".into())
            ),
            Style::default().fg(Color::Cyan),
        )
    } else if app.live {
        Span::styled(
            format!("ativo (pid {})", app.agent.pid.map(|pid| pid.to_string()).unwrap_or_else(|| "?".into())),
            Style::default().fg(Color::Green),
        )
    } else if app.is_enrolled() {
        Span::styled("parado", Style::default().fg(Color::Yellow))
    } else {
        Span::styled("sem inscrição", Style::default().fg(Color::Yellow))
    };

    let inscription = match &app.node {
        Some(node) => format!(
            "{} ({}) · {}",
            node.node_name,
            node.overlay_ip,
            node.control_url
        ),
        None => "— (use j para entrar na malha)".into(),
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled(format!("{:<14}", "estado"), Style::default().fg(Color::DarkGray)),
            state,
            Span::styled(
                if app.agent.managed {
                    "  · iniciado pela TUI"
                } else {
                    ""
                },
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        kv("inscrição", truncate(&inscription, 96)),
        kv(
            "auto-start",
            if app.auto_start {
                "ligado — reabrir a TUI inicia e reconecta o agente"
            } else {
                "desligado — reabrir a TUI não inicia o agente"
            },
        ),
        kv(
            "configuração",
            format!(
                "UDP {} · MTU {} · force-relay {} · TUN {} · expose {}",
                app.settings.udp_port,
                app.settings.mtu,
                if app.settings.force_relay { "sim" } else { "não" },
                if app.settings.tun_name.trim().is_empty() {
                    "—"
                } else {
                    app.settings.tun_name.trim()
                },
                if app.settings.expose.trim().is_empty() {
                    "—"
                } else {
                    app.settings.expose.trim()
                }
            ),
        ),
        kv("log", truncate(&app.log_path.display().to_string(), 96)),
        kv(
            "sessão",
            format!(
                "iniciada {} · {} linhas de log carregadas",
                app.session_started
                    .with_timezone(&chrono::Local)
                    .format("%d/%m %H:%M:%S"),
                app.agent.log.len()
            ),
        ),
    ];
    if let Some(error) = &app.agent.last_error {
        lines.push(Line::from(Span::styled(
            format!("último erro: {error}"),
            Style::default().fg(Color::Red),
        )));
    }
    lines.push(Line::from(Span::styled(
        "s iniciar · X parar · R reiniciar · e ajustes · a auto-start · d doctor · i serviço",
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(block(" Agente local ")),
        chunks[0],
    );

    let mut log_lines: Vec<Line> = Vec::new();
    if app.agent.log.is_empty() {
        log_lines.push(Line::from(Span::styled(
            if app.live {
                "log vazio — se o agente veio do serviço, veja journalctl/Console.app"
            } else {
                "log vazio ou agente ainda sem saída"
            },
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for line in app.agent.log.iter().rev().take(12).rev() {
            let style = if line.contains("ERROR") || line.contains("error") {
                Style::default().fg(Color::Red)
            } else if line.contains("WARN") || line.contains("warn") {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::Gray)
            };
            log_lines.push(Line::from(Span::styled(truncate(line, 160), style)));
        }
    }
    frame.render_widget(
        Paragraph::new(log_lines).block(block(" Saída do agente (últimas linhas) ")),
        chunks[1],
    );
}

fn draw_services(frame: &mut Frame, area: Rect, app: &mut App) {
    let extensions_height = if area.height >= 22 { 5 } else { 3 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7),
            Constraint::Min(4),
            Constraint::Length(extensions_height),
        ])
        .split(area);

    let gui = app.jobs.iter().find(|job| job.label == "gui");
    let gui_text = match gui {
        Some(job) if app_job_alive(job) => format!("aberta (pid {})", job.pid),
        Some(job) => format!("encerrando (pid {})", job.pid),
        None => "fechada".into(),
    };
    let lines = vec![
        kv(
            "GUI local",
            format!(
                "{gui_text} · g abre no navegador · G fecha"
            ),
        ),
        kv(
            "serviço",
            "systemd/launchd do agente · i instalar · u remover · t status",
        ),
        kv("diagnóstico", "d executa wm doctor"),
        kv(
            "root",
            if actions::is_root() {
                "sim"
            } else {
                "não — iniciar/instalar exige sudo wm tui"
            },
        ),
        kv(
            "painel admin",
            "p abre a URL do control no navegador (login de admin)",
        ),
    ];
    frame.render_widget(
        Paragraph::new(lines).block(block(" Ferramentas ")),
        chunks[0],
    );

    let header = Row::new(vec!["Job", "PID", "Estado", "Log"]).style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );
    let rows: Vec<Row> = app
        .jobs
        .iter()
        .map(|job| {
            let alive = app_job_alive(job);
            let state = if job.stopping {
                Span::styled("parando", Style::default().fg(Color::Cyan))
            } else if alive {
                Span::styled("ativo", Style::default().fg(Color::Green))
            } else {
                Span::styled("encerrado", Style::default().fg(Color::DarkGray))
            };
            Row::new(vec![
                Cell::from(job.label.clone()),
                Cell::from(job.pid.to_string()),
                Cell::from(state),
                Cell::from(truncate(&job.log.display().to_string(), 48)),
            ])
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Length(12),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Min(20),
        ],
    )
    .header(header)
    .row_highlight_style(Style::default().bg(HIGHLIGHT).add_modifier(Modifier::BOLD))
    .highlight_symbol("▸ ")
    .block(block(if app.jobs.is_empty() {
        " Jobs em segundo plano ".to_string()
    } else {
        format!(" Jobs em segundo plano ({}) ", app.jobs.len())
    }));
    frame.render_stateful_widget(table, chunks[1], &mut app.jobs_table);
    let offset = app.jobs_table.offset();
    let jobs = app.jobs.len();
    register_table_rows(app, chunks[1], offset, jobs, MouseAction::SelectJob);

    if app.jobs.is_empty() {
        empty_message(
            frame,
            chunks[1],
            "nenhum host/serve/gui em segundo plano; use :host/:serve ou o menu de ações",
        );
    }

    let mut extension_lines = Vec::new();
    if app.extensions.is_empty() {
        extension_lines.push(Line::from(Span::styled(
            if app.live {
                "nenhuma extensão ativa neste nó"
            } else {
                "sem agente: extensões indisponíveis"
            },
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for extension in &app.extensions {
            let state_style = match extension.state {
                wm_proto::ExtensionState::Running => Style::default().fg(Color::Green),
                wm_proto::ExtensionState::Error => Style::default().fg(Color::Red),
                wm_proto::ExtensionState::Starting => Style::default().fg(Color::Cyan),
                wm_proto::ExtensionState::Stopped => Style::default().fg(Color::DarkGray),
            };
            let urls = if extension.public_urls.is_empty() {
                String::new()
            } else {
                format!("  ·  {}", extension.public_urls.join(", "))
            };
            extension_lines.push(Line::from(vec![
                Span::styled(
                    format!("{:<14}", extension.id),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{:<10}", extension.state.to_string()),
                    state_style,
                ),
                Span::styled(extension.driver.clone(), Style::default().fg(Color::Gray)),
                Span::raw(truncate(
                    &format!(
                        "{}{}{}",
                        extension
                            .message
                            .as_deref()
                            .map(|message| format!("  {message}"))
                            .unwrap_or_default(),
                        urls,
                        ""
                    ),
                    90,
                )),
            ]));
        }
    }
    frame.render_widget(
        Paragraph::new(extension_lines).block(block(stale(app, "Extensões"))),
        chunks[2],
    );
}

fn app_job_alive(job: &crate::app::Job) -> bool {
    crate::agent::pid_alive(job.pid)
}

fn draw_console(frame: &mut Frame, area: Rect, app: &mut App) {
    let height = area.height.saturating_sub(2) as usize;
    let total = app.console.len();
    let max_scroll = total.saturating_sub(height);
    if app.console_scroll > max_scroll {
        app.console_scroll = max_scroll;
    }
    let scroll = app.console_scroll;
    let bottom = total.saturating_sub(scroll);
    let start = bottom.saturating_sub(height);
    let mut lines = Vec::new();
    for line in &app.console[start..bottom.min(total)] {
        let glyph_style = Style::default().fg(line.level.color());
        let text_style = if line.previous {
            Style::default().fg(Color::DarkGray)
        } else if line.level == Level::Output {
            Style::default().fg(Color::Gray)
        } else {
            Style::default().fg(line.level.color())
        };
        lines.push(Line::from(vec![
            Span::styled(
                if line.previous { "↺ " } else { "  " },
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(line.level.glyph(), glyph_style),
            Span::raw(" "),
            Span::styled(truncate(&line.text, 400), text_style),
        ]));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "sem eventos ainda. Pressione : para executar um comando wm.",
            Style::default().fg(Color::DarkGray),
        )));
    }
    let scroll_hint = if app.console_scroll > 0 {
        format!(" · -{} acima", app.console_scroll)
    } else {
        String::new()
    };
    frame.render_widget(
        Paragraph::new(lines).block(block(format!(
            " Console / eventos · {} linhas{scroll_hint} ",
            total
        ))),
        area,
    );
}

fn draw_help(frame: &mut Frame, area: Rect, app: &mut App) {
    let lines: Vec<Line> = vec![
        Line::from(Span::styled(
            "atalhos e fluxo",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("  1…8 / Tab / ← →   troca de aba"),
        Line::from("  ↑ ↓ / j k          navega (Console: rola o histórico)"),
        Line::from("  PgUp / PgDn · Home/End   rolagem do Console e da Ajuda"),
        Line::from("  :                  comando wm (ex.: status, share 3000 --name demo)"),
        Line::from("  ?                  esta ajuda"),
        Line::from("  q / Ctrl+C         sai — o agente continua rodando e reconecta"),
        Line::from(""),
        Line::from(Span::styled(
            "mouse",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("  clique             abas, botões de ação, linhas de tabela e modais"),
        Line::from("  duplo clique       abre share, probe do peer ou log do job"),
        Line::from("  roda do mouse      rola listas, console, ajuda e troca de aba"),
        Line::from("  clique direito     abre o menu com todas as ações (ou tecla m)"),
        Line::from("  formulários        clique no campo alterna opção; botões Confirmar/Cancelar"),
        Line::from("  Shift+arrastar     seleciona texto; --no-mouse desliga a captura"),
        Line::from(""),
        Line::from(Span::styled(
            "primeiros passos",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("  sem inscrição:      j → cole o código wmc_ (ou URL + token)"),
        Line::from("  inscrito, parado:   s inicia o agente em segundo plano"),
        Line::from("  inscrito, ativo:    a tela se atualiza sozinha; n renomeia o nó"),
        Line::from(""),
        Line::from(Span::styled(
            "agente",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("  s iniciar · X parar · R reiniciar · e ajustes · a auto-start"),
        Line::from("  l log no console · d doctor · i instalar serviço · g GUI · p painel"),
        Line::from("  ao sair com q, o processo iniciado pela TUI continua ativo"),
        Line::from("  (log em agent.log) até :down, X, wm down ou reboot"),
        Line::from(""),
        Line::from(Span::styled(
            "serviços",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("  Enter log do job · g/G GUI local · p painel admin · x parar job"),
        Line::from("  i instalar serviço · u remover · t status · d doctor"),
        Line::from(""),
        Line::from(Span::styled(
            "peers e shares",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("  Peers:   p ping · o probe · x reconectar · c copiar overlay"),
        Line::from("  Shares:  n publicar · Enter abrir · c copiar · r renomear"),
        Line::from("           e ligar/desligar · d remover (duas vezes)"),
        Line::from("  n (Peers/Início): renomear este nó"),
        Line::from(""),
        Line::from(Span::styled(
            "console (aba 7 ou :)",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("  status · peers · shares · share 3000 --name demo · open demo"),
        Line::from("  resolve nome · ping peer · probe peer 80 · shares"),
        Line::from("  up · down · join <url> --token … · host · serve · gui · doctor"),
        Line::from("  service status · service install --mode agent · version"),
        Line::from("  comandos longos (up/join/host/serve/gui) rodam em segundo plano"),
        Line::from("  sem shell: aspas simples/duplas agrupam argumentos"),
        Line::from(""),
        Line::from(Span::styled(
            "sessão persistente",
            Style::default().fg(Color::Cyan),
        )),
        Line::from("  aba, histórico, configurações, jobs e o último retrato da malha"),
        Line::from("  ficam em tui-state.json (0600). Eventos em tui-events.jsonl."),
        Line::from("  Tokens e convites são redigidos antes de gravar."),
    ];
    let height = area.height.saturating_sub(2) as usize;
    let max = lines.len().saturating_sub(height);
    if app.help_scroll > max {
        app.help_scroll = max;
    }
    let start = app.help_scroll.min(max);
    let visible: Vec<Line> = lines
        .into_iter()
        .skip(start)
        .take(height.max(1))
        .collect();
    let suffix = if max > 0 {
        format!(" · {}/{} ", start + 1, max + 1)
    } else {
        String::new()
    };
    frame.render_widget(
        Paragraph::new(visible).block(block(format!(" Ajuda{suffix}"))),
        area,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(area);

    if let Some(input) = &app.input {
        app.input_rect = chunks[0];
        let mut prompt = vec![
            Span::styled(": ", Style::default().fg(Color::Cyan)),
            Span::raw(input.value.clone()),
            Span::styled("█", Style::default().add_modifier(Modifier::BOLD)),
        ];
        if let Some(error) = &input.error {
            prompt.push(Span::styled(
                format!("   {error}"),
                Style::default().fg(Color::Red),
            ));
        }
        frame.render_widget(Paragraph::new(Line::from(prompt)), chunks[0]);

        let last_word = input
            .value
            .split_whitespace()
            .last()
            .unwrap_or("")
            .to_string();
        let names = if input.value.trim().contains(' ') {
            Vec::new()
        } else {
            crate::command::suggestions(&last_word)
        };
        let mut hint = vec![Span::styled(
            "Enter executa · Tab completa · ↑↓ histórico · Esc fecha   ",
            Style::default().fg(Color::DarkGray),
        )];
        if !names.is_empty() {
            hint.push(Span::styled(
                names.join(" · "),
                Style::default().fg(Color::Cyan),
            ));
        }
        if let [only] = names.as_slice() {
            if let Some(description) = crate::command::description(only) {
                hint.push(Span::styled(
                    format!("   — {description}"),
                    Style::default().fg(Color::DarkGray),
                ));
            }
        }
        frame.render_widget(Paragraph::new(Line::from(hint)), chunks[1]);
        return;
    }

    let notice_line = if let Some(notice) = app.last_notice() {
        Line::from(vec![
            Span::styled(
                format!(
                    "{} ",
                    notice
                        .at
                        .with_timezone(&chrono::Local)
                        .format("%H:%M:%S")
                ),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                format!("{} ", notice.level.glyph()),
                Style::default().fg(notice.level.color()),
            ),
            Span::styled(notice.text.clone(), Style::default().fg(notice.level.color())),
        ])
    } else if app.live {
        Line::from(Span::styled(
            "conectado ao agente · dados atualizando a cada segundo",
            Style::default().fg(Color::DarkGray),
        ))
    } else {
        Line::from(Span::styled(
            "pronto · pressione : para comandos, ? para ajuda",
            Style::default().fg(Color::DarkGray),
        ))
    };
    frame.render_widget(Paragraph::new(notice_line), chunks[0]);

    let hint = match app.tab {
        TAB_STATUS => {
            if !app.is_enrolled() {
                " j entrar na malha · :comando · ? ajuda · q sair"
            } else {
                " s iniciar · X parar · R reiniciar · e ajustes · a auto · g GUI · ? ajuda"
            }
        }
        TAB_PEERS => " p ping · o probe · x reconectar · c copiar · n renomear nó · ? ajuda",
        TAB_SHARES => " n publicar · Enter abrir · c copiar · r renomear · e on/off · d remover",
        TAB_TOPOLOGY => " Tab abas · ? ajuda · q sair (agente continua)",
        TAB_AGENT => " s iniciar · X parar · R reiniciar · e ajustes · l log · i serviço",
        TAB_SERVICES => " Enter log · g/G GUI · p painel · x para job · i/u serviço · t status",
        TAB_CONSOLE => " : comando · j/k rolar · PgUp/PgDn · Home/End · c limpar",
        TAB_HELP => " j/k rolar · :comando · q sair (agente continua)",
        _ => " :comando · 1…8 abas · q sair (agente continua)",
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            hint,
            Style::default().fg(Color::DarkGray),
        ))),
        chunks[1],
    );
}

fn draw_form(frame: &mut Frame, area: Rect, app: &mut App) {
    let Some(form) = app.form.as_ref() else {
        return;
    };
    let height = (form.fields.len() as u16 + 8).min(area.height.saturating_sub(2));
    let width = area.width.saturating_sub(8).clamp(44, 86).min(area.width);
    let rect = centered(area, width, height);
    let inner = rect.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);
    let content = sections[0];
    let buttons = sections[1];
    let mouse = app.mouse;

    let (lines, hits, title) = {
        let form = app.form.as_ref().expect("form is open");
        let mut lines: Vec<Line> = Vec::new();
        let mut hits: Vec<(Rect, MouseAction)> = Vec::new();
        for (index, field) in form.fields.iter().enumerate() {
            let focused = index == form.focus;
            let row_y = content.y.saturating_add(index as u16);
            let row_rect = Rect {
                x: content.x,
                y: row_y,
                width: content.width,
                height: 1,
            };
            if row_y < content.y.saturating_add(content.height) {
                let action = match field.kind {
                    FieldKind::Bool => MouseAction::FormToggle(index),
                    FieldKind::Choice => MouseAction::FormCycle(index, true),
                    _ => MouseAction::FormFocus(index),
                };
                hits.push((row_rect, action));
            }
            let hovered = mouse.is_some_and(|(column, row)| {
                crate::app::rect_contains(row_rect, column, row)
            });
            let label_style = if focused {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else if hovered {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let value = match field.kind {
                FieldKind::Choice => format!("‹ {} ›", field.value),
                FieldKind::Bool => format!("[{}]", field.value),
                _ => {
                    if field.kind == FieldKind::Text && field.max > 0 {
                        fit_tail(&field.value, 60)
                    } else {
                        field.value.clone()
                    }
                }
            };
            let value_style = if focused {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else if hovered {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(Color::Gray)
            };
            let mut spans = vec![
                Span::styled(if focused { "▸ " } else { "  " }, label_style),
                Span::styled(format!("{:<18}", field.label), label_style),
                Span::styled(value, value_style),
            ];
            if focused && !matches!(field.kind, FieldKind::Choice | FieldKind::Bool) {
                spans.push(Span::styled(
                    "  █",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            lines.push(Line::from(spans));
        }
        lines.push(Line::from(""));
        if let Some(field) = form.focused() {
            lines.push(Line::from(Span::styled(
                format!("  {}", field.hint),
                Style::default().fg(Color::DarkGray),
            )));
        }
        if let Some(note) = form.note {
            lines.push(Line::from(Span::styled(
                format!("  {note}"),
                Style::default().fg(Color::DarkGray),
            )));
        }
        if let Some(error) = &form.error {
            lines.push(Line::from(Span::styled(
                format!("  ✗ {error}"),
                Style::default().fg(Color::Red),
            )));
        }
        lines.push(Line::from(Span::styled(
            "  clique: foca/alterna · Tab/↑↓ campo · Enter envia · Esc cancela",
            Style::default().fg(Color::DarkGray),
        )));
        (lines, hits, form.title)
    };

    frame.render_widget(Clear, rect);
    frame.render_widget(
        Block::default()
            .title(format!(" {title} "))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
        rect,
    );
    // Fields must stay one line per row: hit regions are computed from the
    // field index, so wrapping would shift clicks onto the wrong field.
    frame.render_widget(Paragraph::new(lines), content);
    draw_button_row(
        frame,
        buttons,
        app,
        &[
            button("Confirmar", MouseAction::FormSubmit),
            button("Cancelar", MouseAction::FormCancel),
        ],
    );
    for (rect, action) in hits {
        app.register_overlay_hit(rect, action);
    }
}

fn draw_confirm(frame: &mut Frame, area: Rect, app: &mut App) {
    let Some(confirm) = app.confirm.as_ref() else {
        return;
    };
    let height = (confirm.lines.len() as u16 + 6).min(area.height.saturating_sub(2));
    let rect = centered(area, 72.min(area.width), height);
    let inner = rect.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);
    let (lines, title) = {
        let confirm = app.confirm.as_ref().expect("confirm is open");
        let mut lines: Vec<Line> = confirm
            .lines
            .iter()
            .map(|line| Line::from(Span::raw(line.clone())))
            .collect();
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(
                "s",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" confirmar   "),
            Span::styled(
                "n",
                Style::default()
                    .fg(Color::Red)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" ou Esc cancela"),
        ]));
        (lines, confirm.title.clone())
    };
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Block::default()
            .title(format!(" {title} "))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow)),
        rect,
    );
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        sections[0],
    );
    draw_button_row(
        frame,
        sections[1],
        app,
        &[
            button("Confirmar", MouseAction::ConfirmYes),
            button("Cancelar", MouseAction::ConfirmNo),
        ],
    );
}

fn draw_menu(frame: &mut Frame, area: Rect, app: &mut App) {
    let Some(menu) = app.menu.as_ref() else {
        return;
    };
    let total = menu.items.len();
    let height = (total as u16 + 4).min(area.height.saturating_sub(2));
    let width = area.width.saturating_sub(8).clamp(44, 78).min(area.width);
    let rect = centered(area, width, height);
    let inner = rect.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);
    let list = sections[0];
    let buttons = sections[1];
    let visible = list.height as usize;
    let (lines, hits, title, offset) = {
        let menu = app.menu.as_ref().expect("menu is open");
        let mut offset = menu.offset;
        if menu.selected < offset {
            offset = menu.selected;
        }
        if visible > 0 && menu.selected >= offset + visible {
            offset = menu.selected + 1 - visible;
        }
        let max_offset = total.saturating_sub(visible);
        offset = offset.min(max_offset);
        let mut lines = Vec::new();
        let mut hits = Vec::new();
        for row in 0..visible {
            let index = offset + row;
            if index >= total {
                break;
            }
            let item = &menu.items[index];
            let selected = index == menu.selected;
            let row_rect = Rect {
                x: list.x,
                y: list.y.saturating_add(row as u16),
                width: list.width,
                height: 1,
            };
            let hovered = app
                .mouse
                .is_some_and(|(column, row)| crate::app::rect_contains(row_rect, column, row));
            let style = if selected {
                Style::default()
                    .fg(Color::Cyan)
                    .bg(Color::Rgb(18, 32, 40))
                    .add_modifier(Modifier::BOLD)
            } else if hovered {
                Style::default().fg(Color::Black).bg(Color::Cyan)
            } else {
                Style::default().fg(Color::Gray)
            };
            lines.push(Line::from(vec![
                Span::styled(if selected { "▸ " } else { "  " }, style),
                Span::styled(
                    truncate(&item.label, list.width.saturating_sub(2) as usize),
                    style,
                ),
            ]));
            hits.push((row_rect, MouseAction::MenuSelect(index)));
        }
        if lines.is_empty() {
            lines.push(Line::from(Span::styled(
                "sem ações disponíveis",
                Style::default().fg(Color::DarkGray),
            )));
        }
        let title = format!(
            " Ações · {}/{} · ↑↓ Enter · Esc ",
            menu.selected.saturating_add(1).min(total.max(1)),
            total
        );
        (lines, hits, title, offset)
    };
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
        rect,
    );
    frame.render_widget(Paragraph::new(lines), list);
    if let Some(menu) = app.menu.as_mut() {
        menu.offset = offset;
    }
    for (rect, action) in hits {
        app.register_overlay_hit(rect, action);
    }
    draw_button_row(
        frame,
        buttons,
        app,
        &[button("Fechar", MouseAction::MenuClose)],
    );
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

fn empty_message(frame: &mut Frame, area: Rect, message: &str) {
    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 2,
    });
    let line = Line::from(Span::styled(
        message,
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), inner);
}

fn fmt_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn temp_root(label: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "wm-tui-ui-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn render(app: &mut App, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn demo_app(label: &str) -> App {
        use uuid::Uuid;
        use wm_proto::{AgentStatus, ExtensionState, ExtensionStatus, NatType, PathKind, PeerStatus, ShareInfo, ShareKind, ShareProtocol};
        let root = temp_root(label);
        let mut app = App::load(root);
        let node_id = Uuid::new_v4();
        app.node = Some(crate::persist::EnrolledNode {
            node_name: "studio".into(),
            overlay_ip: "10.66.0.1".into(),
            control_url: "https://mesh.exemplo.com".into(),
            network_name: "wired-magic".into(),
            created_at: Some(chrono::Utc::now()),
        });
        app.status = Some(AgentStatus {
            version: "0.1.0".into(),
            node_id: Some(node_id),
            node_name: Some("studio".into()),
            overlay_ip: Some("10.66.0.1".parse().unwrap()),
            control_url: Some("https://mesh.exemplo.com".into()),
            control_connected: true,
            relay_connected: true,
            tun_name: Some("utun7".into()),
            udp_port: Some(41641),
            nat_type: NatType::Cone,
            peers_total: 2,
            peers_online: 2,
            uptime_secs: 4_212,
            map_version: 9,
            last_error: None,
        });
        app.live = true;
        app.cached_at = Some(chrono::Utc::now());
        app.peers = vec![
            PeerStatus {
                id: Uuid::new_v4(),
                name: "mac-01".into(),
                overlay_ip: "10.66.0.12".parse().unwrap(),
                path: PathKind::Direct,
                endpoint: Some("203.0.113.9:41641".into()),
                rtt_ms: Some(4),
                last_handshake_secs: Some(2),
                rx_bytes: 1_048_576,
                tx_bytes: 2_097_152,
                relay_online: true,
                last_error: None,
            },
            PeerStatus {
                id: Uuid::new_v4(),
                name: "mac-02".into(),
                overlay_ip: "10.66.0.13".parse().unwrap(),
                path: PathKind::Relay,
                endpoint: None,
                rtt_ms: Some(18),
                last_handshake_secs: Some(9),
                rx_bytes: 512,
                tx_bytes: 256,
                relay_online: true,
                last_error: None,
            },
        ];
        app.shares = vec![
            ShareInfo {
                id: Uuid::new_v4(),
                name: "demo".into(),
                node_id,
                node_name: "studio".into(),
                overlay_ip: "10.66.0.1".parse().unwrap(),
                port: 3000,
                kind: ShareKind::Web,
                protocol: ShareProtocol::Tcp,
                description: "preview do app".into(),
                enabled: true,
                online: true,
                public_url: None,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            },
            ShareInfo {
                id: Uuid::new_v4(),
                name: "cs2".into(),
                node_id: Uuid::new_v4(),
                node_name: "mac-01".into(),
                overlay_ip: "10.66.0.12".parse().unwrap(),
                port: 27015,
                kind: ShareKind::Game,
                protocol: ShareProtocol::Both,
                description: "servidor de teste".into(),
                enabled: true,
                online: true,
                public_url: None,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            },
        ];
        app.extensions = vec![ExtensionStatus {
            id: "cloudflared".into(),
            name: "Túnel".into(),
            driver: "cloudflared".into(),
            state: ExtensionState::Running,
            message: None,
            public_urls: vec!["https://demo.trycloudflare.com".into()],
            details: serde_json::Value::Null,
            updated_at: chrono::Utc::now(),
        }];
        app.jobs.push(crate::app::Job {
            pid: std::process::id(),
            label: "gui".into(),
            log: std::path::PathBuf::from("/tmp/gui.log"),
            stopping: false,
        });
        app.agent.pid = Some(4242);
        app.agent.managed = true;
        app.agent.log = vec![
            "INFO agente iniciado".into(),
            "WARN nat cone para 10.66.0.12".into(),
            "INFO mapa v9 (2 peers)".into(),
        ];
        app.console.push(crate::app::ConsoleLine {
            level: Level::Good,
            text: "agente ativo (pid 4242)".into(),
            previous: false,
        });
        app.console.push(crate::app::ConsoleLine {
            level: Level::Output,
            text: "share demo publicado em 10.66.0.1:3000".into(),
            previous: false,
        });
        app.table.select(Some(0));
        app.shares_table.select(Some(0));
        app.jobs_table.select(Some(0));
        app
    }

    #[test]
    #[ignore = "preview manual: cargo test -p wm-tui print_tui_preview -- --ignored --nocapture"]
    fn print_tui_preview() {
        let mut app = demo_app("preview");
        for tab in 0..TABS.len() {
            app.tab = tab;
            println!("===== {} =====", TABS[tab]);
            println!("{}", render(&mut app, 120, 32));
        }
        app.tab = TAB_AGENT;
        app.form = Some(crate::app::join_form());
        println!("===== formulário =====");
        println!("{}", render(&mut app, 120, 32));
        app.form = None;
        println!("===== 80x24: Início =====");
        app.tab = TAB_STATUS;
        println!("{}", render(&mut app, 80, 24));
        println!("===== 80x24: Serviços =====");
        app.tab = TAB_SERVICES;
        println!("{}", render(&mut app, 80, 24));
        println!("===== menu =====");
        crate::open_menu(&mut app);
        println!("{}", render(&mut app, 110, 32));
        app.menu = None;
        println!("===== confirmação =====");
        app.confirm = Some(crate::app::Confirm {
            title: "Parar o agente".into(),
            lines: vec![
                "Isto encerra a conexão da malha neste nó.".into(),
                "Os shares e a inscrição continuam gravados.".into(),
            ],
            action: crate::app::ConfirmAction::StopAgent,
        });
        println!("{}", render(&mut app, 110, 32));
        std::fs::remove_dir_all(&app.root).unwrap();
    }

    #[test]
    fn mouse_hit_regions_are_inside_the_frame_and_do_not_overlap() {
        let mut app = demo_app("mouse-hits");
        for tab in 0..TABS.len() {
            app.tab = tab;
            let _ = render(&mut app, 80, 24);
            assert!(
                app.hits
                    .iter()
                    .any(|hit| hit.action == MouseAction::OpenMenu),
                "aba {tab} sem botão de menu"
            );
            for hit in &app.hits {
                assert!(hit.rect.width > 0 && hit.rect.height > 0);
                assert!(
                    hit.rect.x + hit.rect.width <= 80 && hit.rect.y + hit.rect.height <= 24,
                    "hit fora da tela: {:?} {:?}",
                    hit.action,
                    hit.rect
                );
            }
            for (index, left) in app.hits.iter().enumerate() {
                for right in app.hits.iter().skip(index + 1) {
                    let overlap = left.rect.x < right.rect.x + right.rect.width
                        && right.rect.x < left.rect.x + left.rect.width
                        && left.rect.y < right.rect.y + right.rect.height
                        && right.rect.y < left.rect.y + left.rect.height;
                    assert!(
                        !overlap,
                        "hits sobrepostos: {:?} {:?} x {:?} {:?}",
                        left.action, left.rect, right.action, right.rect
                    );
                }
            }
        }
        std::fs::remove_dir_all(&app.root).unwrap();
    }

    #[test]
    fn tiny_terminal_renders_all_tabs_and_overlays() {
        let mut app = demo_app("tiny");
        for size in [(1, 1), (20, 8), (40, 12), (60, 18)] {
            for tab in 0..TABS.len() {
                app.tab = tab;
                let _ = render(&mut app, size.0, size.1);
            }
            app.form = Some(crate::app::new_share_form());
            let _ = render(&mut app, size.0, size.1);
            app.form = None;
            app.confirm = Some(crate::app::Confirm {
                title: "Teste".into(),
                lines: vec!["linha".into()],
                action: crate::app::ConfirmAction::StopAgent,
            });
            let _ = render(&mut app, size.0, size.1);
            app.confirm = None;
            crate::open_menu(&mut app);
            let _ = render(&mut app, size.0, size.1);
            app.menu = None;
        }
        std::fs::remove_dir_all(&app.root).unwrap();
    }

    #[test]
    fn onboarding_is_visible_without_enrollment() {
        let root = temp_root("onboarding");
        let mut app = App::load(root.clone());
        app.tab = TAB_STATUS;
        let text = render(&mut app, 100, 30);
        assert!(text.contains("ainda não entrou na malha"));
        assert!(text.contains("pressione j"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn every_tab_renders_without_panic() {
        let root = temp_root("tabs");
        let mut app = App::load(root.clone());
        for tab in 0..TABS.len() {
            app.tab = tab;
            let text = render(&mut app, 100, 30);
            assert!(!text.trim().is_empty(), "tab {tab} vazia");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn agent_tab_shows_lifecycle_actions_and_journal() {
        let root = temp_root("agent");
        let mut app = App::load(root.clone());
        app.tab = TAB_AGENT;
        app.agent.log = vec!["INFO começando".into(), "ERROR exemplo".into()];
        let text = render(&mut app, 110, 32);
        assert!(text.contains("Agente local"));
        assert!(text.contains("auto-start"));
        assert!(text.contains("ERROR exemplo"));
        assert!(text.contains("parado"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn services_console_help_and_shares_render() {
        let root = temp_root("misc");
        let mut app = App::load(root.clone());
        for tab in [TAB_SERVICES, TAB_CONSOLE, TAB_HELP, TAB_SHARES] {
            app.tab = tab;
            let text = render(&mut app, 100, 30);
            assert!(text.contains("Console / eventos") || text.contains("Ajuda") || text.contains("Serviços") || text.contains("Shares"));
        }
        app.tab = TAB_CONSOLE;
        app.push_console(Level::Good, "linha de teste");
        let text = render(&mut app, 100, 30);
        assert!(text.contains("linha de teste"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn form_and_confirm_overlay_are_drawn() {
        let root = temp_root("form");
        let mut app = App::load(root.clone());
        app.form = Some(crate::app::join_form());
        let text = render(&mut app, 100, 30);
        assert!(text.contains("Entrar na malha"));
        assert!(text.contains("código wmc_"));
        app.form = None;
        app.confirm = Some(crate::app::Confirm {
            title: "Parar o agente".into(),
            lines: vec!["teste".into()],
            action: crate::app::ConfirmAction::StopAgent,
        });
        let text = render(&mut app, 100, 30);
        assert!(text.contains("Parar o agente"));
        assert!(text.contains("confirmar"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn share_details_use_selected_share() {
        let root = temp_root("share");
        let mut app = App::load(root.clone());
        app.tab = TAB_SHARES;
        app.shares = vec![wm_proto::ShareInfo {
            id: uuid::Uuid::new_v4(),
            name: "demo".into(),
            node_id: uuid::Uuid::new_v4(),
            node_name: "mac-01".into(),
            overlay_ip: "10.66.0.12".parse().unwrap(),
            port: 3000,
            kind: wm_proto::ShareKind::Web,
            protocol: wm_proto::ShareProtocol::Tcp,
            description: "preview".into(),
            enabled: true,
            online: true,
            public_url: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }];
        app.shares_table.select(Some(0));
        let text = render(&mut app, 110, 30);
        assert!(text.contains("demo"));
        assert!(text.contains("10.66.0.12:3000"));
        assert!(text.contains("wm://demo"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
