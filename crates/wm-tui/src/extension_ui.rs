//! Local presentation preferences never activate a node service.
use crate::app::{App, Field, Form, FormAction, Level, TAB_CONSOLE};

impl App {
    /// UI-only startup work. Fullscreen exec is always an explicit user action.
    pub fn autoload_tabs(&self) -> Vec<(usize, String, wm_proto::TuiMode)> {
        self.extension_tabs.iter().enumerate().filter(|(_, tab)| {
            tab.spec.mode != wm_proto::TuiMode::Exec && !self.autoloaded.contains(&tab.id)
                && self.extension_preferences.extensions.get(&tab.id)
                    .is_some_and(|prefs| prefs.favorite && prefs.autoload)
        }).map(|(i, tab)| (i, tab.id.clone(), tab.spec.mode)).collect()
    }

    pub fn active_catalog(&self) -> &[crate::catalog::Entry] {
        if self.extension_section == 2 { &self.discovery_catalog } else { &self.local_catalog }
    }

    pub fn extension_id(&self) -> Option<String> {
        if self.catalog_focus { self.selected_catalog().map(|e| e.id.clone()) }
        else { self.selected_extension().map(|e| e.id.clone()) }
    }

    pub fn select_extension_section(&mut self, section: usize) {
        self.extension_section = section.min(3);
        self.catalog_focus = self.extension_section != 0;
        self.catalog_table.select((!self.active_catalog().is_empty()).then_some(0));
        if section == 2 { self.discover_extensions(); }
    }

    pub fn discover_extensions(&mut self) {
        let selected = self.selected_catalog().map(|e| e.id.clone());
        let cwd = std::env::current_dir().unwrap_or_else(|_| self.root.clone());
        let result = crate::catalog::discover(&self.root, &cwd);
        self.discovery_catalog = result.entries;
        self.discovery_error = result.error;
        self.catalog_table.select(selected.and_then(|id| self.discovery_catalog.iter().position(|e| e.id == id))
            .or_else(|| (!self.discovery_catalog.is_empty()).then_some(0)));
        self.rebuild_extension_tabs();
    }

    pub fn edit_extension_settings(&mut self) {
        let Some(id) = self.extension_id() else { return; };
        let prefs = self.extension_preferences.extensions.get(&id).cloned().unwrap_or_default();
        let options = serde_json::to_string(&prefs.options).unwrap_or_default();
        let mut form = Form::new(" Settings da extensão ", FormAction::ExtensionSettings(id), vec![
            Field::boolean("favorito", prefs.favorite, "restaurar aba por ID; nunca ativa serviço"),
            Field::boolean("autoload", prefs.autoload, "carregar UI favorita ao restaurar; nunca exec fullscreen"),
            Field::text("opções TUI (JSON)", &options, 8192, "objeto string:string; somente apresentação, sem segredos"),
        ]);
        form.note = Some("Opções locais da TUI, não configuração de serviço nem credenciais.");
        self.form = Some(form);
    }

    pub fn extension_logs(&mut self, tx: &std::sync::mpsc::Sender<crate::Msg>) {
        if self.extension_logs_pending { return; }
        let Some(id) = self.extension_id() else { return; };
        let mut lines = vec![format!("[{id}] estado/erros observados; consultando logs retidos do agente")];
        if let Some(view) = self.extension_catalog.iter().find(|e| e.id == id) {
            lines.push(format!("[{id}] active={} configured={} status={:?}", view.active, view.configured, view.status));
        }
        if let Some(error) = self.pane_errors.get(&id) { lines.push(format!("[{id}] pane: {error}")); }
        if let Some(error) = self.extension_tabs.iter().find(|t| t.id == id).and_then(|t| t.error.as_ref()) {
            lines.push(format!("[{id}] snapshot: {error}"));
        }
        if lines.len() == 1 { lines.push(format!("[{id}] nenhum status ou erro disponível")); }
        for line in lines {
            self.push_console(Level::Info, crate::persist::redact_input(&crate::persist::sanitize(&line)));
        }
        self.tab = TAB_CONSOLE;
        self.extension_logs_pending = true;
        let socket = self.socket.clone();
        let tx = tx.clone();
        std::thread::spawn(move || {
            let result = crate::actions::local_call(&socket, "extension_logs", serde_json::json!({"id": id, "limit": 100}));
            let _ = tx.send(crate::Msg::ExtensionLogs { id, result });
        });
    }

    pub fn finish_extension_logs(&mut self, id: &str, result: Result<serde_json::Value, String>) {
        self.extension_logs_pending = false;
        let clean = |value: &str| crate::persist::redact_input(&crate::persist::sanitize(value));
        match result {
            Ok(value) => {
                if let Some(entries) = value.get("entries").and_then(serde_json::Value::as_array) {
                    if entries.is_empty() { self.push_console(Level::Info, format!("[{id}] sem logs retidos; captura começa com o agente e não inclui histórico anterior")); }
                    for entry in entries.iter().take(256) {
                        let source = entry.get("source").and_then(serde_json::Value::as_str).unwrap_or("log");
                        let message = entry.get("message").and_then(serde_json::Value::as_str).unwrap_or("");
                        self.push_console(Level::Output, clean(&format!("[{id}] {source}: {message}")));
                    }
                } else { self.push_console(Level::Warn, format!("[{id}] resposta de logs inválida")); }
            }
            Err(_) => self.push_console(Level::Warn, format!("[{id}] logs do agente indisponíveis (offline, bridge ou versão sem suporte)")),
        }
    }

    pub fn poll_snapshot(&mut self) {
        let Some(rx) = &self.snapshot_rx else { return; };
        match rx.try_recv() {
            Ok((id, spec, result)) => {
                self.snapshot_rx = None;
                if let Some(tab) = self.extension_tabs.iter_mut().find(|t| t.id == id && t.spec == spec) {
                    match result { Ok(doc) => { tab.document = Some(doc); tab.error = None; }, Err(e) => tab.error = Some(e) }
                    tab.loaded_at = Some(std::time::Instant::now());
                }
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => { self.snapshot_rx = None; }
            Err(_) => {}
        }
    }
}

/// Drains both pipes concurrently; the UI thread never waits for a subprocess.
/// Kill the process group on timeout so descendants cannot keep pipes open.
pub fn snapshot(mut command: std::process::Command, timeout: std::time::Duration) -> Result<wm_proto::TuiDocument, String> {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    command.stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0);
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    use std::os::fd::AsRawFd;
    use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
    fn drain(
        mut pipe: impl Read + AsRawFd + Send + 'static,
        done: Arc<AtomicBool>,
    ) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            // Nonblocking reads also bound completion when a detached child
            // inherits a pipe outside the process group we can terminate.
            let fd = pipe.as_raw_fd();
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
                return Vec::new();
            }
            let mut result = Vec::new();
            let mut buf = [0u8; 8192];
            loop {
                match pipe.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let keep = n.min((1024usize * 1024).saturating_sub(result.len()));
                        result.extend_from_slice(&buf[..keep]);
                        if keep < n && done.load(Ordering::Acquire) { break; }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        if done.load(Ordering::Acquire) { break; }
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
            result
        })
    }
    let done = Arc::new(AtomicBool::new(false));
    let stdout = drain(child.stdout.take().unwrap(), done.clone());
    let stderr = drain(child.stderr.take().unwrap(), done.clone());
    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() < timeout => std::thread::sleep(std::time::Duration::from_millis(20)),
            _ => break None,
        }
    };
    unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL); }
    let _ = child.wait();
    done.store(true, Ordering::Release);
    let out = stdout.join().unwrap_or_default();
    let err = stderr.join().unwrap_or_default();
    let status = status.ok_or("snapshot excedeu timeout")?;
    if !status.success() { return Err(format!("{status}: {}", crate::persist::sanitize(&String::from_utf8_lossy(&err)))); }
    serde_json::from_slice(&out).map_err(|e| format!("JSON da TUI inválido: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_times_out_and_decodes_json() {
        let mut command = std::process::Command::new("sh");
        command.args(["-c", "sleep 10"]);
        let at = std::time::Instant::now();
        assert!(snapshot(command, std::time::Duration::from_millis(40)).unwrap_err().contains("timeout"));
        assert!(at.elapsed() < std::time::Duration::from_secs(2));
        let mut command = std::process::Command::new("printf");
        command.arg(r#"{"title":"hello"}"#);
        assert_eq!(snapshot(command, std::time::Duration::from_secs(1)).unwrap().title, "hello");
        let mut command = std::process::Command::new("sh");
        command.args(["-c", "i=0; while [ $i -lt 12000 ]; do printf 'stderr-pressure-test\\n' >&2; i=$((i+1)); done; printf '{\"title\":\"drained\"}'"]);
        assert_eq!(snapshot(command, std::time::Duration::from_secs(3)).unwrap().title, "drained");
        let mut command = std::process::Command::new("sh");
        command.args(["-c", "printf '\\033[31mfailure' >&2; exit 7"]);
        let error = snapshot(command, std::time::Duration::from_secs(1)).unwrap_err();
        assert!(error.contains("failure"));
        assert!(!error.contains('\u{1b}'));
        let mut command = std::process::Command::new("printf");
        command.arg("not JSON");
        assert!(snapshot(command, std::time::Duration::from_secs(1)).unwrap_err().contains("JSON"));
    }

    fn entry(id: &str, mode: wm_proto::TuiMode) -> crate::catalog::Entry {
        crate::catalog::Entry {
            id: id.into(),
            tui: Some(wm_proto::TuiSpec {
                tab: id.into(), mode, command: "printf".into(), args: vec![],
                refresh_secs: 5, options: Default::default(),
            }),
            ..Default::default()
        }
    }

    #[test]
    fn restore_waits_for_id_and_autoload_never_executes_fullscreen() {
        let root = tempfile::tempdir().unwrap();
        let mut app = App::load(root.path().into());
        app.restore_extension_id = Some("later".into());
        for id in ["later", "fullscreen", "not-favorite"] {
            app.extension_preferences.extensions.insert(id.into(), wm_ext::ExtensionPreferences {
                favorite: id != "not-favorite", autoload: true, ..Default::default()
            });
        }
        app.rebuild_extension_tabs();
        assert_eq!(app.restore_extension_id.as_deref(), Some("later"));
        app.local_catalog = vec![entry("later", wm_proto::TuiMode::Snapshot),
            entry("fullscreen", wm_proto::TuiMode::Exec), entry("not-favorite", wm_proto::TuiMode::Pane)];
        app.rebuild_extension_tabs();
        assert_eq!(app.selected_extension_tab().unwrap().id, "later");
        assert!(app.restore_extension_id.is_none());
        let work = app.autoload_tabs();
        assert_eq!(work.len(), 1);
        assert_eq!(work[0].1, "later");
        app.autoloaded.insert("later".into());
        assert!(app.autoload_tabs().is_empty());
        assert!(app.pending_tui_exec.is_none());
        assert!(app.panes.is_empty());
    }

    #[test]
    fn offline_refresh_keeps_section_focus_and_discovery_selection() {
        let root = tempfile::tempdir().unwrap();
        let mut app = App::load(root.path().into());
        app.local_catalog = vec![entry("local", wm_proto::TuiMode::Snapshot)];
        app.select_extension_section(0);
        app.apply_snapshot(crate::Snapshot { local_catalog: app.local_catalog.clone(), ..Default::default() });
        assert!(!app.catalog_focus);
        app.extension_section = 2;
        app.catalog_focus = true;
        app.discovery_catalog = vec![entry("a", wm_proto::TuiMode::Snapshot), entry("b", wm_proto::TuiMode::Snapshot)];
        app.catalog_table.select(Some(1));
        app.apply_snapshot(crate::Snapshot::default());
        assert_eq!(app.selected_catalog().unwrap().id, "b");
        assert!(app.catalog_focus);
    }

    #[test]
    fn worker_rejects_stale_specs_and_logs_are_sanitized() {
        let root = tempfile::tempdir().unwrap();
        let mut app = App::load(root.path().into());
        app.local_catalog = vec![entry("custom", wm_proto::TuiMode::Snapshot)];
        app.rebuild_extension_tabs();
        let old = app.extension_tabs[0].spec.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        app.snapshot_rx = Some(rx);
        app.local_catalog[0].tui.as_mut().unwrap().args.push("changed".into());
        app.rebuild_extension_tabs();
        tx.send(("custom".into(), old, Ok(wm_proto::TuiDocument { title: "stale".into(), ..Default::default() }))).unwrap();
        app.poll_snapshot();
        assert!(app.snapshot_rx.is_none());
        assert!(app.extension_tabs[0].document.is_none());
        app.select_extension_section(1);
        app.pane_errors.insert("custom".into(), "\u{1b}[31mfailed --token secret-value".into());
        let (log_tx, _log_rx) = std::sync::mpsc::channel();
        app.extension_logs(&log_tx);
        let text = app.console.iter().map(|line| line.text.as_str()).collect::<Vec<_>>().join("\n");
        assert!(text.contains("[custom] pane:"));
        assert!(!text.contains("\u{1b}"));
        assert!(!text.contains("secret-value"));
        assert_eq!(app.tab, TAB_CONSOLE);
    }

    #[test]
    fn retained_logs_are_bounded_and_sanitized_without_changing_current_view() {
        let root = tempfile::tempdir().unwrap();
        let mut app = App::load(root.path().into());
        let tab = app.tab;
        app.extension_logs_pending = true;
        app.finish_extension_logs("custom", Ok(serde_json::json!({"entries": [
            {"source":"stdout", "message":"ready\u{1b}[31m --token should-not-display"}
        ]})));
        assert!(!app.extension_logs_pending);
        assert_eq!(app.tab, tab);
        let text = app.console.iter().map(|line| line.text.as_str()).collect::<Vec<_>>().join("\n");
        assert!(text.contains("[custom] stdout: ready"));
        assert!(!text.contains("should-not-display"));
        assert!(!text.contains('\u{1b}'));
        app.finish_extension_logs("custom", Err("untrusted sensitive error".into()));
        assert!(!app.console.iter().any(|line| line.text.contains("untrusted sensitive error")));
    }

    #[test]
    fn discovered_assets_resolve_even_when_local_catalog_has_embedded_id() {
        let root = tempfile::tempdir().unwrap();
        let assets = tempfile::tempdir().unwrap();
        let mut app = App::load(root.path().into());
        let mut embedded = entry("custom", wm_proto::TuiMode::Snapshot);
        embedded.path = "<embutida>".into();
        app.local_catalog = vec![embedded.clone()];
        embedded.path = assets.path().display().to_string();
        app.discovery_catalog = vec![embedded];
        app.rebuild_extension_tabs();
        let (_, _, cwd) = crate::resolve_tui_command(&app, &app.extension_tabs[0]);
        assert_eq!(cwd.as_deref(), Some(assets.path()));
    }

    #[test]
    fn settings_errors_do_not_overwrite_preferences() {
        let root = tempfile::tempdir().unwrap();
        let mut app = App::load(root.path().into());
        app.local_catalog = vec![entry("custom", wm_proto::TuiMode::Snapshot)];
        app.select_extension_section(3);
        let (tx, _rx) = std::sync::mpsc::channel();
        for options in ["bad json", r#"{"password":"secret"}"#, r#"{"theme":false}"#] {
            app.edit_extension_settings();
            app.form.as_mut().unwrap().fields[2].value = options.into();
            crate::actions::submit_form(&mut app, &tx);
            assert!(app.form.as_ref().unwrap().error.is_some());
            assert!(wm_ext::ExtensionPreferencesStore::load(root.path()).unwrap().extensions.is_empty());
        }
        assert!(app.pending_tui_exec.is_none());
        assert!(app.panes.is_empty());
    }

    #[test]
    fn selection_survives_reordering_and_settings_roundtrip() {
        let root = tempfile::tempdir().unwrap();
        let mut app = App::load(root.path().into());
        app.local_catalog = crate::catalog::scan(root.path()).entries;
        app.rebuild_extension_tabs();
        let id = app.extension_tabs.last().unwrap().id.clone();
        app.tab = crate::app::TABS.len() + app.extension_tabs.len() - 1;
        let mut entry = app.local_catalog.iter().find(|e| e.tui.is_some()).unwrap().clone();
        entry.id = "aaa-new".into();
        app.local_catalog.push(entry);
        app.rebuild_extension_tabs();
        assert_eq!(app.selected_extension_tab().unwrap().id, id);
        app.select_extension_section(3);
        app.edit_extension_settings();
        let id = app.extension_id().unwrap();
        let form = app.form.as_mut().unwrap();
        form.fields[0].cycle(true);
        form.fields[1].cycle(true);
        form.fields[2].value = r#"{"theme":"dark"}"#.into();
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::actions::submit_form(&mut app, &tx);
        assert!(app.form.is_none());
        let store = wm_ext::ExtensionPreferencesStore::load(root.path()).unwrap();
        assert!(store.extensions[&id].favorite);
        assert_eq!(store.extensions[&id].options["theme"], "dark");
        assert!(app.pending_tui_exec.is_none());
        assert!(app.panes.is_empty());
    }
}
