//! TUI debug dashboard (`wm debug`).
//!
//! This is the default debug experience: it samples the local host and the
//! `wm` process directly, so it works without a sidecar, a network port, or a
//! running `debug_gui`. Discovery of local extension manifests is folded into
//! the same sampler; `wm debug discover` gives the detailed/one-shot view.

use std::path::Path;
use std::time::Duration;

use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table};
use super::core::terminal::{State, SAMPLE_INTERVAL};
pub fn run(root: &Path) -> anyhow::Result<()> {
    let mut state = State::new(root);
    state.sample();
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut state);
    ratatui::restore();
    result
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    state: &mut State,
) -> anyhow::Result<()> {
    loop {
        if state.last_sample.elapsed() >= SAMPLE_INTERVAL {
            state.sample();
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
            KeyCode::Char('q') | KeyCode::Esc => break,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
            KeyCode::Char('r') => state.sample(),
            _ => {}
        }
    }
    Ok(())
}

fn draw(frame: &mut Frame, state: &State) {
    let [header, stats, wm, cores, processes, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(4),
        Constraint::Length(5),
        Constraint::Length(5),
        Constraint::Min(6),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_header(frame, header, state);
    draw_stats(frame, stats, state);
    draw_wm(frame, wm, state);
    draw_cores(frame, cores, state);
    draw_processes(frame, processes, state);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "q/Esc sai · r atualiza · discovery: wm debug discover",
            Style::default().fg(Color::DarkGray),
        ))),
        footer,
    );
}

fn draw_header(frame: &mut Frame, area: Rect, state: &State) {
    let line = Line::from(vec![
        Span::styled(
            format!(" {} ", state.hostname),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("· "),
        Span::styled(state.os.clone(), Style::default().fg(Color::Gray)),
        Span::raw(" · "),
        Span::styled(
            format!("up {}", format_duration(state.uptime_secs)),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(vec![
            line,
            Line::from(Span::styled(
                state.status.clone(),
                Style::default().fg(Color::DarkGray),
            )),
        ])
        .block(
            Block::default()
                .title(" debug ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        ),
        area,
    );
}

fn draw_stats(frame: &mut Frame, area: Rect, state: &State) {
    let mem_pct = if state.memory_total > 0 {
        state.memory_used as f64 / state.memory_total as f64 * 100.0
    } else {
        0.0
    };
    let lines = vec![
        Line::from(vec![
            label("CPU "),
            value(format!("{:>5.1}%", state.cpu)),
            Span::raw("   "),
            label("MEM "),
            value(format!(
                "{:>5.1}%  {} / {}",
                mem_pct,
                format_bytes(state.memory_used),
                format_bytes(state.memory_total)
            )),
        ]),
        Line::from(vec![
            label("LOAD "),
            value(format!(
                "{:.2} {:.2} {:.2}",
                state.load_one, state.load_five, state.load_fifteen
            )),
            Span::raw("   "),
            label("EXTENSÕES "),
            value(format!(
                "{} local(is) · {} nova(s)",
                state.extensions.entries.len(),
                state.extensions.new_count
            )),
        ]),
    ];
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(" performance ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        ),
        area,
    );
}

fn draw_wm(frame: &mut Frame, area: Rect, state: &State) {
    let mut lines = Vec::new();
    match &state.wm {
        Some(wm) => lines.push(Line::from(vec![
            label("wm (TUI) "),
            value(format!("pid {} · {} · ", wm.pid, wm.state)),
            value(format!("{:.1}% CPU · {} RSS", wm.cpu, format_bytes(wm.memory))),
        ])),
        None => lines.push(Line::from(Span::styled(
            "processo wm não encontrado",
            Style::default().fg(Color::DarkGray),
        ))),
    }
    if let Some(parent) = &state.wm_parent {
        lines.push(Line::from(vec![
            label("wm (agente) "),
            value(format!(
                "pid {} · {} · {:.1}% CPU · {} RSS",
                parent.pid,
                parent.state,
                parent.cpu,
                format_bytes(parent.memory)
            )),
        ]));
    }
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(" processo wm ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        ),
        area,
    );
}

fn draw_cores(frame: &mut Frame, area: Rect, state: &State) {
    let mut lines = Vec::new();
    for (index, usage) in state.per_core.iter().enumerate() {
        let width = ((*usage as f64 / 100.0) * 24.0).round() as usize;
        let bar = format!("{}{}", "█".repeat(width.min(24)), "░".repeat(24 - width.min(24)));
        lines.push(Line::from(vec![
            label(format!("#{index:<2} ")),
            Span::styled(
                bar,
                Style::default().fg(if *usage >= 85.0 {
                    Color::Red
                } else if *usage >= 70.0 {
                    Color::Yellow
                } else {
                    Color::Cyan
                }),
            ),
            Span::raw(format!(" {usage:>5.1}%")),
        ]));
        if lines.len() >= 3 {
            // The card is intentionally compact; per-core detail beyond the
            // first rows is still visible in `debug_gui`.
            break;
        }
    }
    if lines.is_empty() {
        lines.push(Line::from("sem dados de CPU"));
    }
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(format!(" núcleos ({}) ", state.per_core.len()))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        ),
        area,
    );
}

fn draw_processes(frame: &mut Frame, area: Rect, state: &State) {
    let rows: Vec<Row> = state
        .processes
        .iter()
        .map(|process| {
            Row::new(vec![
                process.pid.to_string(),
                process.name.clone(),
                format!("{:.1}%", process.cpu),
                format_bytes(process.memory),
                process.state.clone(),
            ])
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Length(7),
            Constraint::Min(16),
            Constraint::Length(7),
            Constraint::Length(10),
            Constraint::Length(10),
        ],
    )
    .header(
        Row::new(vec!["PID", "PROCESSO", "CPU", "RSS", "ESTADO"]).style(
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ),
    )
    .block(
        Block::default()
            .title(" top processos ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    frame.render_widget(table, area);
}

fn label(value: impl Into<String>) -> Span<'static> {
    Span::styled(
        value.into(),
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )
}

fn value(value: impl Into<String>) -> Span<'static> {
    Span::styled(value.into(), Style::default().fg(Color::Gray))
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
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

fn format_duration(seconds: u64) -> String {
    let days = seconds / 86400;
    let hours = (seconds % 86400) / 3600;
    let minutes = (seconds % 3600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn tui_draw_does_not_panic() {
        let mut state = State::new(Path::new("/nonexistent/wm-debug-tui-test"));
        state.sample();
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &state)).unwrap();
    }
}
