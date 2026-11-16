//! Embedded pseudo-terminal panes for extension-provided TUIs.
//!
//! The central TUI owns a PTY, spawns the extension command inside it, feeds
//! its output through a `vt100` parser and forwards keyboard bytes. The
//! extension is free to implement any terminal UI (ratatui, curses, shell);
//! the central TUI only provides the terminal surface.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc::Sender;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use portable_pty::{
    Child, CommandBuilder, MasterPty, PtySize, native_pty_system,
};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use crate::Msg;

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

pub struct Pane {
    /// Distinguishes reader events from successive PTYs sharing an extension ID.
    pub generation: u64,
    pub master: Box<dyn MasterPty + Send>,
    pub writer: Box<dyn Write + Send>,
    pub child: Box<dyn Child + Send + Sync>,
    pub parser: vt100::Parser,
    pub rows: u16,
    pub cols: u16,
    pub exited: bool,
    pub exit: Option<String>,
}

impl Pane {
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        id: String,
        command: &str,
        args: &[String],
        cwd: Option<&Path>,
        env: &BTreeMap<String, String>,
        rows: u16,
        cols: u16,
        tx: Sender<Msg>,
    ) -> Result<Self, String> {
        let rows = rows.max(1);
        let cols = cols.max(1);
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|err| err.to_string())?;

        let mut builder = CommandBuilder::new(command);
        builder.args(args);
        builder.env("TERM", "xterm-256color");
        builder.env("COLORTERM", "truecolor");
        for (key, value) in env {
            builder.env(key, value);
        }
        if let Some(cwd) = cwd {
            builder.cwd(cwd);
        }
        let child = pair
            .slave
            .spawn_command(builder)
            .map_err(|err| err.to_string())?;
        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|err| err.to_string())?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|err| err.to_string())?;

        let pane_id = id.clone();
        let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
        thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx
                            .send(Msg::PaneData {
                                id: pane_id.clone(),
                                generation,
                                bytes: buffer[..n].to_vec(),
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = tx.send(Msg::PaneClosed { id: pane_id, generation });
        });

        Ok(Self {
            generation,
            master: pair.master,
            writer,
            child,
            parser: vt100::Parser::new(rows, cols, 2_000),
            rows,
            cols,
            exited: false,
            exit: None,
        })
    }

    /// In-memory PTY substitute for event-routing tests; never spawns a process.
    #[cfg(test)]
    pub(crate) fn test_fixture(generation: u64) -> Self {
        #[derive(Debug)]
        struct FakePty;
        impl MasterPty for FakePty {
            fn resize(&self, _: PtySize) -> anyhow::Result<()> { Ok(()) }
            fn get_size(&self) -> anyhow::Result<PtySize> {
                Ok(PtySize { rows: 4, cols: 40, pixel_width: 0, pixel_height: 0 })
            }
            fn try_clone_reader(&self) -> anyhow::Result<Box<dyn Read + Send>> {
                Ok(Box::new(std::io::empty()))
            }
            fn take_writer(&self) -> anyhow::Result<Box<dyn Write + Send>> {
                Ok(Box::new(std::io::sink()))
            }
            #[cfg(unix)]
            fn process_group_leader(&self) -> Option<libc::pid_t> { None }
            #[cfg(unix)]
            fn as_raw_fd(&self) -> Option<std::os::fd::RawFd> { None }
            #[cfg(unix)]
            fn tty_name(&self) -> Option<std::path::PathBuf> { None }
        }
        #[derive(Debug)]
        struct FakeChild;
        impl portable_pty::ChildKiller for FakeChild {
            fn kill(&mut self) -> std::io::Result<()> { Ok(()) }
            fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
                Box::new(Self)
            }
        }
        impl Child for FakeChild {
            fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> { Ok(None) }
            fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
                Ok(portable_pty::ExitStatus::with_exit_code(0))
            }
            fn process_id(&self) -> Option<u32> { None }
        }
        Self {
            generation, master: Box::new(FakePty), writer: Box::new(std::io::sink()),
            child: Box::new(FakeChild), parser: vt100::Parser::new(4, 40, 0),
            rows: 4, cols: 40, exited: false, exit: None,
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        if rows == 0 || cols == 0 || (rows == self.rows && cols == self.cols) {
            return;
        }
        self.rows = rows;
        self.cols = cols;
        self.parser.screen_mut().set_size(rows, cols);
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }

    pub fn write(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    pub fn poll(&mut self) {
        if self.exited {
            return;
        }
        match self.child.try_wait() {
            Ok(Some(status)) => {
                self.exited = true;
                self.exit = Some(match status.signal() {
                    Some(signal) => format!("sinal {signal}"),
                    None => format!("código {}", status.exit_code()),
                });
            }
            Ok(None) => {}
            Err(err) => {
                self.exited = true;
                self.exit = Some(err.to_string());
            }
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        render_screen(self.parser.screen(), frame, area);
    }
}

pub fn render_screen(screen: &vt100::Screen, frame: &mut Frame, area: Rect) {
    if area.width == 0 || area.height == 0 {
            return;
        }
        let (screen_rows, screen_cols) = screen.size();
        let rows = area.height.min(screen_rows);
        let cols = area.width.min(screen_cols);
        let (cursor_row, cursor_col) = screen.cursor_position();
        let cursor_visible = !screen.hide_cursor();
        let buffer = frame.buffer_mut();
        for row in 0..rows {
            for col in 0..cols {
                let Some(cell) = screen.cell(row, col) else {
                    continue;
                };
                let Some(target) = buffer.cell_mut((area.x + col, area.y + row)) else {
                    continue;
                };
                let contents = cell.contents();
                target.set_symbol(if contents.is_empty() { " " } else { contents });
                let mut style = Style::default();
                if let Some(color) = convert_color(cell.fgcolor()) {
                    style = style.fg(color);
                }
                if let Some(color) = convert_color(cell.bgcolor()) {
                    style = style.bg(color);
                }
                if cell.bold() {
                    style = style.add_modifier(Modifier::BOLD);
                }
                if cell.italic() {
                    style = style.add_modifier(Modifier::ITALIC);
                }
                if cell.underline() {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }
                if cell.inverse() {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                if cursor_visible && cursor_row == row && cursor_col == col {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                target.set_style(style);
            }
        }
}

impl Drop for Pane {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn convert_color(color: vt100::Color) -> Option<Color> {
    match color {
        vt100::Color::Default => None,
        vt100::Color::Idx(index) => Some(Color::Indexed(index)),
        vt100::Color::Rgb(red, green, blue) => Some(Color::Rgb(red, green, blue)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    #[ignore = "exige /dev/ptmx; rode fora do sandbox"]
    fn pty_spawns_and_streams_output() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut pane = Pane::start(
            "test".into(),
            "/bin/sh".into(),
            &["-c".into(), "printf hello-pty".into()],
            None,
            &BTreeMap::new(),
            10,
            40,
            tx,
        )
        .unwrap();
        let mut output = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(crate::Msg::PaneData { bytes, .. }) => output.extend(bytes),
                Ok(_) => {}
                Err(_) => {}
            }
            if output.windows(9).any(|window| window == b"hello-pty") {
                break;
            }
        }
        pane.feed(&output);
        assert!(
            String::from_utf8_lossy(&output).contains("hello-pty"),
            "saída do PTY: {:?}",
            output
        );
    }

    #[test]
    fn terminal_screen_is_rendered_into_the_frame() {
        let mut parser = vt100::Parser::new(4, 20, 0);
        parser.process(b"hello\r\nworld");
        let backend = TestBackend::new(20, 4);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_screen(parser.screen(), frame, frame.area()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let text: String = (0..4)
            .map(|row| {
                (0..20)
                    .map(|col| buffer[(col, row)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("hello"));
        assert!(text.contains("world"));
    }
}
