//! Actions triggered from the TUI: forms, agent lifecycle, local API calls,
//! share management and the command console.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Instant;

use serde_json::{Value, json};
use uuid::Uuid;
use wm_proto::{ShareKind, ShareProtocol, validate_share_description, validate_share_name};

use crate::agent;
use crate::app::{
    App, Confirm, ConfirmAction, Form, FormAction, Level, TAB_HELP, TAB_PEERS, TAB_SHARES,
    TAB_STATUS,
};
use crate::command::{self, Kind};
use crate::persist;
use crate::Msg;

pub fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

/// One request/response round trip to the local agent socket.
pub fn local_call(socket: &Path, method: &str, params: Value) -> Result<Value, String> {
    use std::io::{BufRead, BufReader, Write};
    let mut stream = std::os::unix::net::UnixStream::connect(socket)
        .map_err(|err| format!("socket {}: {err}", socket.display()))?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(9)))
        .ok();
    let request = json!({ "id": 1, "method": method, "params": params });
    let mut line = serde_json::to_vec(&request).map_err(|err| err.to_string())?;
    line.push(b'\n');
    stream.write_all(&line).map_err(|err| err.to_string())?;
    stream.flush().ok();
    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader
        .read_line(&mut response)
        .map_err(|err| err.to_string())?;
    let parsed: wm_proto::LocalResponse =
        serde_json::from_str(&response).map_err(|err| format!("resposta inválida: {err}"))?;
    if parsed.ok {
        Ok(parsed.result.unwrap_or(Value::Null))
    } else {
        Err(parsed.error.unwrap_or_else(|| "erro no agente".into()))
    }
}

// ---------------------------------------------------------------------------
// Forms
// ---------------------------------------------------------------------------

fn restore_error(app: &mut App, mut form: Form, error: impl Into<String>) {
    form.error = Some(error.into());
    app.form = Some(form);
}

fn parse_port(raw: &str, what: &str) -> Result<u16, String> {
    let port: u16 = raw
        .trim()
        .parse()
        .map_err(|_| format!("{what}: porta inválida"))?;
    if port == 0 {
        return Err(format!("{what}: porta 0 não vale"));
    }
    Ok(port)
}

/// Validates and dispatches the currently open form.
pub fn submit_form(app: &mut App, tx: &Sender<Msg>) {
    let Some(mut form) = app.form.take() else {
        return;
    };
    let action = form.action.clone();
    match action {
        FormAction::Join => {
            let code = form.value_owned(0);
            let url = form.value_owned(1);
            let token = form.value_owned(2);
            let name = form.value_owned(3);
            // Accept a wmc_ code in either field, and a wm://join link as-is.
            let raw_code = if !code.is_empty() {
                code.clone()
            } else if url.starts_with("wmc_") {
                url.clone()
            } else {
                String::new()
            };
            let raw_url = if !code.is_empty() && !code.starts_with("wmc_") {
                code.clone()
            } else if raw_code.is_empty() {
                url.clone()
            } else {
                String::new()
            };
            let url_has_token = raw_url.starts_with("wm://join");
            if raw_code.is_empty()
                && (raw_url.is_empty() || (token.is_empty() && !url_has_token))
            {
                form.focus = if raw_code.is_empty() { 1 } else { 0 };
                return restore_error(
                    app,
                    form,
                    "informe o código wmc_ ou URL + token do convite",
                );
            }
            if app.node.is_some() {
                return restore_error(
                    app,
                    form,
                    "este nó já está inscrito; use :join --force para entrar em outra malha",
                );
            }
            if app.node_error.is_some() {
                return restore_error(
                    app,
                    form,
                    "não foi possível ler agent-state.json (rode com sudo wm tui)",
                );
            }
            let mut args = vec!["join".to_string()];
            if !raw_code.is_empty() {
                args.push("--code".into());
                args.push(raw_code);
            } else {
                args.push(raw_url);
                if !token.is_empty() {
                    args.push("--token".into());
                    args.push(token);
                }
            }
            if !name.is_empty() {
                args.push("--name".into());
                args.push(name);
            }
            args.push("--no-start".into());
            let root = app.root.clone();
            let exe = app.exe.clone();
            let socket = app.socket.clone();
            let settings = app.settings.clone();
            let tx = tx.clone();
            app.notify(Level::Info, "inscrevendo este nó…");
            thread::spawn(move || join_then_start(root, exe, args, socket, settings, tx));
        }
        FormAction::NewShare => {
            let port = match parse_port(form.value(0), "share") {
                Ok(port) => port,
                Err(err) => {
                    form.focus = 0;
                    return restore_error(app, form, err);
                }
            };
            let raw_name = form.value_owned(1);
            let name = if raw_name.is_empty() {
                format!("port-{port}")
            } else {
                match validate_share_name(&raw_name) {
                    Ok(name) => name,
                    Err(err) => {
                        form.focus = 1;
                        return restore_error(app, form, err);
                    }
                }
            };
            let kind = ShareKind::parse(form.value(2)).unwrap_or(ShareKind::Web);
            let protocol = match form.value(3) {
                "tcp" => Some(ShareProtocol::Tcp),
                "udp" => Some(ShareProtocol::Udp),
                "both" => Some(ShareProtocol::Both),
                _ => None,
            };
            let description = form.value_owned(4);
            let description = match validate_share_description(&description) {
                Ok(description) => description,
                Err(err) => {
                    form.focus = 4;
                    return restore_error(app, form, err);
                }
            };
            let enabled = form.bool_at(5);
            let mut params = json!({
                "name": name,
                "port": port,
                "kind": kind,
                "description": description,
                "enabled": enabled,
            });
            if let Some(protocol) = protocol {
                params["protocol"] = serde_json::to_value(protocol).unwrap_or(Value::Null);
            }
            let socket = app.socket.clone();
            let label = name.clone();
            app.notify(
                Level::Info,
                format!("publicando {name} na porta {port}…"),
            );
            let tx = tx.clone();
            thread::spawn(move || {
                match local_call(&socket, "share_create", params) {
                    Ok(value) => {
                        let address = value
                            .get("address")
                            .and_then(Value::as_str)
                            .unwrap_or("—")
                            .to_string();
                        let _ = tx.send(Msg::Notify(
                            Level::Good,
                            format!("share {label} publicado em {address}"),
                        ));
                    }
                    Err(err) => {
                        let _ = tx.send(Msg::Notify(
                            Level::Error,
                            format!("share {label}: {err}"),
                        ));
                    }
                }
            });
        }
        FormAction::RenameShare(id) => {
            let name = form.value_owned(0);
            let name = match validate_share_name(&name) {
                Ok(name) => name,
                Err(err) => return restore_error(app, form, err),
            };
            let socket = app.socket.clone();
            app.notify(Level::Info, format!("renomeando share para {name}…"));
            let tx = tx.clone();
            thread::spawn(move || {
                let params = json!({ "id": id, "name": name });
                match local_call(&socket, "share_update", params) {
                    Ok(_) => {
                        let _ = tx.send(Msg::Notify(
                            Level::Good,
                            format!("share renomeado para {name}"),
                        ));
                    }
                    Err(err) => {
                        let _ = tx.send(Msg::Notify(
                            Level::Error,
                            format!("renomear share: {err}"),
                        ));
                    }
                }
            });
        }
        FormAction::RenameNode => {
            let name = form.value_owned(0);
            if name.is_empty() {
                form.focus = 0;
                return restore_error(app, form, "nome vazio");
            }
            let socket = app.socket.clone();
            app.notify(Level::Info, format!("renomeando nó para {name}…"));
            let tx = tx.clone();
            thread::spawn(move || {
                match local_call(&socket, "rename_node", json!({ "name": name })) {
                    Ok(value) => {
                        let applied = value
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or(&name)
                            .to_string();
                        let _ = tx.send(Msg::Notify(
                            Level::Good,
                            format!("nó renomeado para {applied}"),
                        ));
                    }
                    Err(err) => {
                        let _ =
                            tx.send(Msg::Notify(Level::Error, format!("renomear nó: {err}")));
                    }
                }
            });
        }
        FormAction::AgentSettings => {
            let udp_port = match parse_port(form.value(0), "UDP") {
                Ok(port) => port,
                Err(err) => {
                    form.focus = 0;
                    return restore_error(app, form, err);
                }
            };
            let mtu: u16 = match form.value(1).parse::<u16>() {
                Ok(mtu) if (576..=9000).contains(&mtu) => mtu,
                _ => {
                    form.focus = 1;
                    return restore_error(app, form, "MTU deve estar entre 576 e 9000");
                }
            };
            let mut settings = app.settings.clone();
            settings.udp_port = udp_port;
            settings.mtu = mtu;
            settings.force_relay = form.bool_at(2);
            settings.expose = form.value_owned(3);
            settings.tun_name = form.value_owned(4);
            if let Err(err) = settings.expose_specs() {
                form.focus = 3;
                return restore_error(app, form, err);
            }
            app.settings = settings;
            app.dirty = true;
            let running = app.live;
            app.notify(
                Level::Good,
                if running {
                    "configurações salvas; reinicie o agente (R) para aplicar"
                } else {
                    "configurações salvas; valem na próxima inicialização"
                },
            );
        }
        FormAction::ProbePort => {
            let port = match parse_port(form.value(0), "probe") {
                Ok(port) => port,
                Err(err) => {
                    form.focus = 0;
                    return restore_error(app, form, err);
                }
            };
            let Some(peer) = app.selected_peer().map(|peer| peer.name.clone()) else {
                return restore_error(app, form, "selecione um peer na aba Peers");
            };
            probe_peer(app, peer, port, tx);
        }
    }
}

/// Runs `join --no-start` in the background, then starts the detached agent.
fn join_then_start(
    root: PathBuf,
    exe: PathBuf,
    args: Vec<String>,
    socket: PathBuf,
    settings: persist::AgentSettings,
    tx: Sender<Msg>,
) {
    let output = Command::new(&exe)
        .arg("--data-dir")
        .arg(&root)
        .args(&args)
        .stdin(Stdio::null())
        .output();
    let output = match output {
        Ok(output) => output,
        Err(err) => {
            let _ = tx.send(Msg::Notify(
                Level::Error,
                format!("não foi possível executar join: {err}"),
            ));
            return;
        }
    };
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if !line.trim().is_empty() {
            let _ = tx.send(Msg::Console(Level::Output, line.to_string()));
        }
    }
    for line in String::from_utf8_lossy(&output.stderr).lines() {
        if !line.trim().is_empty() {
            let _ = tx.send(Msg::Console(Level::Warn, line.to_string()));
        }
    }
    if !output.status.success() {
        let _ = tx.send(Msg::Notify(
            Level::Error,
            format!(
                "inscrição falhou ({}); veja a saída no console",
                output
                    .status
                    .code()
                    .map(|code| format!("código {code}"))
                    .unwrap_or_else(|| "sem código".into())
            ),
        ));
        return;
    }
    let node = persist::load_enrolled(&root).unwrap_or(None);
    let _ = tx.send(Msg::Joined(node));
    match agent::args_for_settings(&settings) {
        Ok(agent_args) => match agent::start(
            &root,
            &exe,
            agent_args,
            &socket,
            &persist::agent_log_path(&root),
            &tx,
        ) {
            Ok(pid) => {
                let _ = tx.send(Msg::Notify(
                    Level::Info,
                    format!("nó inscrito; iniciando agente em segundo plano (pid {pid})…"),
                ));
            }
            Err(err) => {
                let _ = tx.send(Msg::Notify(
                    Level::Error,
                    format!("inscrito, mas o agente não iniciou: {err}"),
                ));
            }
        },
        Err(err) => {
            let _ = tx.send(Msg::Notify(Level::Error, format!("configuração inválida: {err}")));
        }
    }
}

// ---------------------------------------------------------------------------
// Agent lifecycle
// ---------------------------------------------------------------------------

pub fn start_agent(app: &mut App, tx: &Sender<Msg>) {
    if app.live || agent::socket_ready(&app.socket) {
        app.live = true;
        app.notify(Level::Info, "o agente já está ativo");
        return;
    }
    if let Some(started) = app.agent.starting {
        app.notify(
            Level::Info,
            format!("o agente já está iniciando ({}s)", started.elapsed().as_secs()),
        );
        return;
    }
    if app.node.is_none() {
        app.notify(
            Level::Warn,
            "este nó ainda não está inscrito; use j para entrar na malha",
        );
        return;
    }
    if !is_root() {
        app.notify(
            Level::Error,
            "iniciar o agente exige root; reabra com sudo wm tui (ou use :join ... --no-start só para inscrever)",
        );
        return;
    }
    if agent::agent_lock_held(&app.root) {
        if app.agent.starting.is_none() {
            app.agent.starting = Some(Instant::now());
        }
        if app.agent.pid.is_none() {
            app.agent.pid = agent::read_pid(&app.root);
        }
        app.notify(
            Level::Info,
            "outro processo já reservou este data-dir; aguardando o agente responder",
        );
        return;
    }
    let args = match agent::args_for_settings(&app.settings) {
        Ok(args) => args,
        Err(err) => {
            app.notify(Level::Error, format!("configuração inválida: {err}"));
            return;
        }
    };
    app.agent.starting = Some(Instant::now());
    app.agent.stopping = None;
    app.agent.last_error = None;
    app.agent.managed = true;
    app.auto_start = true;
    app.dirty = true;
    match agent::start(
        &app.root,
        &app.exe,
        args,
        &app.socket,
        &app.log_path,
        tx,
    ) {
        Ok(pid) => {
            app.agent.pid = Some(pid);
            app.notify(
                Level::Info,
                format!("iniciando agente em segundo plano (pid {pid}); ele continua após q"),
            );
        }
        Err(err) => {
            app.agent.starting = None;
            app.agent.last_error = Some(err.to_string());
            app.notify(Level::Error, format!("não foi possível iniciar o agente: {err}"));
        }
    }
}

pub fn stop_agent_now(app: &mut App, tx: &Sender<Msg>, keep_auto_start: bool) {
    let live = app.live || agent::socket_ready(&app.socket);
    if !live && app.agent.starting.is_none() && app.agent.stopping.is_none() {
        app.notify(Level::Info, "o agente já está parado");
        return;
    }
    app.agent.stopping = Some(Instant::now());
    if !keep_auto_start {
        app.auto_start = false;
    }
    app.dirty = true;
    app.notify(
        Level::Warn,
        "parando o agente… (a conexão da malha será encerrada)",
    );
    agent::stop_agent(
        &app.root,
        &app.socket,
        app.agent.pid,
        tx,
    );
}

pub fn confirm_stop_agent(app: &mut App) {
    let live = app.live || agent::socket_ready(&app.socket);
    if !live && app.agent.starting.is_none() {
        app.notify(Level::Info, "o agente já está parado");
        return;
    }
    app.confirm = Some(Confirm {
        title: "Parar o agente".into(),
        lines: vec![
            "Isto encerra a conexão da malha neste nó.".into(),
            "Os shares e a inscrição continuam gravados.".into(),
            "O agente não reinicia sozinho ao reabrir a TUI.".into(),
            "Se ele roda como serviço, o supervisor pode reiniciá-lo.".into(),
        ],
        action: ConfirmAction::StopAgent,
    });
}

pub fn confirm_restart_agent(app: &mut App) {
    let live = app.live || agent::socket_ready(&app.socket);
    if !live && app.agent.starting.is_none() {
        app.notify(Level::Warn, "o agente está parado; use s para iniciar");
        return;
    }
    app.confirm = Some(Confirm {
        title: "Reiniciar o agente".into(),
        lines: vec![
            "O agente é parado e iniciado de novo com as configurações atuais.".into(),
            "A conexão volta em alguns segundos.".into(),
        ],
        action: ConfirmAction::RestartAgent,
    });
}

pub fn confirm_stop_job(app: &mut App, pid: u32, label: &str) {
    app.confirm = Some(Confirm {
        title: format!("Parar {label}"),
        lines: vec![
            format!("Encerra o processo em segundo plano (pid {pid})."),
            "O log continua disponível na aba Serviços.".into(),
        ],
        action: ConfirmAction::StopJob(pid),
    });
}

pub fn confirm_stop_selected_job(app: &mut App) {
    match app.selected_job().map(|job| (job.pid, job.label.clone())) {
        Some((pid, label)) => confirm_stop_job(app, pid, &label),
        None => app.notify(Level::Warn, "nenhum job selecionado"),
    }
}

pub fn confirm_stop_gui(app: &mut App) {
    match app
        .jobs
        .iter()
        .find(|job| job.label == "gui")
        .map(|job| (job.pid, job.label.clone()))
    {
        Some((pid, label)) => confirm_stop_job(app, pid, &label),
        None => app.notify(Level::Info, "a GUI local não está aberta"),
    }
}

pub fn perform_confirm(app: &mut App, action: ConfirmAction, tx: &Sender<Msg>) {
    match action {
        ConfirmAction::StopAgent => stop_agent_now(app, tx, false),
        ConfirmAction::RestartAgent => {
            app.restart_pending = true;
            stop_agent_now(app, tx, true);
        }
        ConfirmAction::StopJob(pid) => {
            if let Some(job) = app.jobs.iter_mut().find(|job| job.pid == pid) {
                job.stopping = true;
                let label = job.label.clone();
                agent::stop_job(pid, &label, tx);
                app.notify(Level::Info, format!("parando {label} (pid {pid})…"));
            }
        }
        ConfirmAction::StartHost => start_host(app, tx),
        ConfirmAction::StartServe => start_serve(app, tx),
        ConfirmAction::DeleteShare(id) => {
            let name = app
                .shares
                .iter()
                .find(|share| share.id == id)
                .map(|share| share.name.clone())
                .unwrap_or_else(|| id.to_string());
            delete_share(app, id, name, tx);
        }
        ConfirmAction::InstallService => {
            if !is_root() {
                app.notify(Level::Error, "instalar o serviço exige root");
            } else {
                let mut args = vec![
                    "service".to_string(),
                    "install".into(),
                    "--mode".into(),
                    "agent".into(),
                    "--udp-port".into(),
                    app.settings.udp_port.to_string(),
                    "--mtu".into(),
                    app.settings.mtu.to_string(),
                ];
                if app.settings.force_relay {
                    args.push("--force-relay".into());
                }
                if !app.settings.tun_name.trim().is_empty() {
                    args.push("--tun-name".into());
                    args.push(app.settings.tun_name.trim().to_string());
                }
                match app.settings.expose_specs() {
                    Ok(specs) => {
                        for spec in specs {
                            args.push("--expose".into());
                            args.push(spec);
                        }
                    }
                    Err(err) => {
                        app.notify(Level::Error, err);
                        return;
                    }
                }
                run_foreground(app, args, "service install", tx);
            }
        }
        ConfirmAction::UninstallService => {
            if !is_root() {
                app.notify(Level::Error, "remover o serviço exige root");
            } else {
                run_foreground(
                    app,
                    vec![
                        "service".into(),
                        "uninstall".into(),
                        "--mode".into(),
                        "agent".into(),
                    ],
                    "service uninstall",
                    tx,
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Console commands
// ---------------------------------------------------------------------------

pub fn open_console(app: &mut App) {
    app.input = Some(crate::app::Input {
        value: String::new(),
        history_index: None,
        error: None,
    });
    app.tab = crate::app::TAB_CONSOLE;
}

pub fn submit_console(app: &mut App, tx: &Sender<Msg>) {
    let Some(input) = app.input.take() else {
        return;
    };
    let line = input.value.trim().to_string();
    if line.is_empty() {
        return;
    }
    app.push_history(&line);
    let launch = match command::parse(&line) {
        Ok(launch) => launch,
        Err(err) => {
            app.notify(Level::Warn, err.clone());
            app.input = Some(crate::app::Input {
                value: line,
                history_index: None,
                error: Some(err),
            });
            return;
        }
    };
    if launch.args.len() == 1 {
        match launch.args[0].as_str() {
            "help" => {
                app.tab = TAB_HELP;
                return;
            }
            "status" => {
                app.tab = TAB_STATUS;
                return;
            }
            "peers" => {
                app.tab = TAB_PEERS;
                return;
            }
            "shares" => {
                app.tab = TAB_SHARES;
                return;
            }
            "join" => {
                app.tab = crate::app::TAB_STATUS;
                app.form = Some(crate::app::join_form());
                return;
            }
            "share" => {
                app.tab = crate::app::TAB_SHARES;
                app.form = Some(crate::app::new_share_form());
                return;
            }
            "down" => {
                stop_agent_now(app, tx, false);
                return;
            }
            _ => {}
        }
    }
    app.push_console(Level::Info, format!("❯ {}", launch.display()));
    match launch.kind {
        Kind::Foreground => match launch.args[0].as_str() {
            "down" => stop_agent_now(app, tx, false),
            _ => run_foreground(app, launch.args.clone(), &launch.args[0], tx),
        },
        Kind::Agent => start_from_console(app, launch.args.clone(), tx),
        Kind::Job(label) => launch_job(app, label, launch.args.clone(), tx),
    }
}

fn start_from_console(app: &mut App, args: Vec<String>, tx: &Sender<Msg>) {
    if app.live || agent::socket_ready(&app.socket) {
        app.live = true;
        app.notify(
            Level::Info,
            "o agente já está ativo; use :down antes de iniciar outro",
        );
        return;
    }
    if let Some(started) = app.agent.starting {
        app.notify(
            Level::Info,
            format!("o agente já está iniciando ({}s)", started.elapsed().as_secs()),
        );
        return;
    }
    if !is_root() {
        app.notify(
            Level::Error,
            "iniciar o agente exige root; reabra com sudo wm tui (ou use :join ... --no-start só para inscrever)",
        );
        return;
    }
    if agent::agent_lock_held(&app.root) {
        if app.agent.starting.is_none() {
            app.agent.starting = Some(Instant::now());
        }
        if app.agent.pid.is_none() {
            app.agent.pid = agent::read_pid(&app.root);
        }
        app.notify(
            Level::Info,
            "outro processo já reservou este data-dir; aguardando o agente responder",
        );
        return;
    }
    let args = if args == ["up".to_string()] {
        match agent::args_for_settings(&app.settings) {
            Ok(args) => args,
            Err(err) => {
                app.notify(Level::Error, err);
                return;
            }
        }
    } else {
        args
    };
    app.agent.starting = Some(Instant::now());
    app.agent.stopping = None;
    app.agent.managed = true;
    app.auto_start = true;
    app.dirty = true;
    match agent::start(
        &app.root,
        &app.exe,
        args,
        &app.socket,
        &app.log_path,
        tx,
    ) {
        Ok(pid) => {
            app.agent.pid = Some(pid);
            app.notify(
                Level::Info,
                format!("processo iniciado (pid {pid}); o agente continua após q"),
            );
        }
        Err(err) => {
            app.agent.starting = None;
            app.notify(Level::Error, format!("não foi possível iniciar: {err}"));
        }
    }
}

fn launch_job(app: &mut App, label: &'static str, args: Vec<String>, tx: &Sender<Msg>) {
    if let Some(job) = app.jobs.iter().find(|job| job.label == label) {
        if agent::pid_alive(job.pid) {
            app.notify(
                Level::Warn,
                format!("{label} já está rodando (pid {}); pare na aba Serviços", job.pid),
            );
            return;
        }
    }
    app.jobs
        .retain(|job| job.label != label || agent::pid_alive(job.pid));
    if app.jobs.is_empty() {
        app.jobs_table.select(None);
    } else if app
        .jobs_table
        .selected()
        .is_none_or(|index| index >= app.jobs.len())
    {
        app.jobs_table.select(Some(app.jobs.len() - 1));
    }
    let log = command::job_log_path(&app.root, label);
    match agent::spawn_job(&app.root, &app.exe, label, &args, &log, tx) {
        Ok(pid) => {
            app.add_job(pid, label.to_string(), log.clone());
            app.notify(
                Level::Info,
                format!(
                    "{label} iniciado em segundo plano (pid {pid}); log em {}",
                    log.display()
                ),
            );
        }
        Err(err) => {
            app.notify(Level::Error, format!("não foi possível iniciar {label}: {err}"));
        }
    }
}

pub fn run_foreground(app: &mut App, args: Vec<String>, label: &str, tx: &Sender<Msg>) {
    match agent::run_foreground(&app.root, &app.exe, &args, label, tx) {
        Ok(pid) => {
            app.children.push(pid);
            app.tab = crate::app::TAB_CONSOLE;
            app.notify(
                Level::Info,
                format!("executando: {}", persist::redact_input(&args.join(" "))),
            );
        }
        Err(err) => {
            app.notify(
                Level::Error,
                format!("não foi possível executar {}: {err}", args.join(" ")),
            );
        }
    }
}

/// Loads the agent log into the console tab.
pub fn view_agent_log(app: &mut App) {
    app.tab = crate::app::TAB_CONSOLE;
    app.push_console(
        Level::Info,
        format!("── log do agente · {} ──", app.log_path.display()),
    );
    let lines = agent::tail(&app.log_path, 64 * 1024);
    if lines.is_empty() {
        app.notify(
            Level::Warn,
            if app.live {
                "log vazio — se o agente veio do serviço, use journalctl/Console.app"
            } else {
                "log vazio ou ainda não criado"
            },
        );
        return;
    }
    for line in lines {
        app.push_console(Level::Output, line);
    }
}

/// Loads the selected background job's log into the console tab.
pub fn view_job_log(app: &mut App) {
    let Some(job) = app.selected_job().cloned() else {
        app.notify(Level::Warn, "nenhum job selecionado");
        return;
    };
    app.tab = crate::app::TAB_CONSOLE;
    app.push_console(
        Level::Info,
        format!(
            "── log de {} (pid {}) · {} ──",
            job.label,
            job.pid,
            job.log.display()
        ),
    );
    let lines = agent::tail(&job.log, 64 * 1024);
    if lines.is_empty() {
        app.notify(
            Level::Warn,
            format!("log vazio ou indisponível: {}", job.log.display()),
        );
        return;
    }
    for line in lines {
        app.push_console(Level::Output, line);
    }
}

pub fn open_gui(app: &mut App, tx: &Sender<Msg>) {
    launch_job(app, "gui", vec!["gui".into(), "--open".into()], tx);
}

/// Opens the control-plane admin panel in the default browser.
/// Toggles one extension for this node through the local agent API. The
/// control plane bumps the per-node activation revision and the agent runs
/// install/activate/deactivate hooks as needed.
pub fn toggle_extension(
    app: &mut App,
    view: &wm_proto::NodeExtensionView,
    tx: &Sender<Msg>,
) {
    if !app.live {
        app.notify(Level::Warn, "agente parado; ative a extensão com o agente no ar");
        return;
    }
    let socket = app.socket.clone();
    let id = view.id.clone();
    let name = view.name.clone();
    let active = !view.active;
    let tx = tx.clone();
    thread::spawn(move || {
        let result = local_call(
            &socket,
            "extension_activation",
            json!({ "id": id, "active": active }),
        );
        let (level, message) = match result {
            Ok(_) if active => (Level::Good, format!("{name} ativada neste nó")),
            Ok(_) => (Level::Good, format!("{name} desativada neste nó")),
            Err(err) => (Level::Error, format!("falha ao alterar {name}: {err}")),
        };
        let _ = tx.send(Msg::Notify(level, message));
    });
}

pub fn open_extensions_panel(app: &mut App) {
    let url = app
        .status
        .as_ref()
        .and_then(|status| status.control_url.clone())
        .or_else(|| app.node.as_ref().map(|node| node.control_url.clone()))
        .filter(|url| !url.trim().is_empty());
    match url {
        Some(url) => {
            let url = format!("{}/extensions", url.trim_end_matches('/'));
            open_external(&url);
            app.notify(Level::Good, format!("abrindo extensões: {url}"));
        }
        None => app.notify(
            Level::Warn,
            "URL do control desconhecida; entre na malha primeiro",
        ),
    }
}

pub fn open_admin_panel(app: &mut App) {
    let url = app
        .status
        .as_ref()
        .and_then(|status| status.control_url.clone())
        .or_else(|| app.node.as_ref().map(|node| node.control_url.clone()))
        .filter(|url| !url.trim().is_empty());
    match url {
        Some(url) => {
            open_external(&url);
            app.notify(Level::Good, format!("abrindo painel de admin: {url}"));
        }
        None => app.notify(
            Level::Warn,
            "URL do control desconhecida; entre na malha primeiro",
        ),
    }
}

pub fn run_doctor(app: &mut App, tx: &Sender<Msg>) {
    run_foreground(app, vec!["doctor".into()], "doctor", tx);
}

pub fn service_status(app: &mut App, tx: &Sender<Msg>) {
    run_foreground(
        app,
        vec!["service".into(), "status".into(), "--mode".into(), "agent".into()],
        "service status",
        tx,
    );
}

/// Starts the default control plane + relay + local node as a background job.
pub fn confirm_start_host(app: &mut App) {
    app.confirm = Some(Confirm {
        title: "Iniciar host local".into(),
        lines: vec![
            "Sobe control plane, relay/STUN e o nó local como job em segundo plano.".into(),
            "Usa os padrões (127.0.0.1:8080) e grava em host.log.".into(),
            "Exige root e pode conflitar com um agente/host já ativo.".into(),
            "Para URL pública, TLS ou senha, use o console: host --help.".into(),
        ],
        action: ConfirmAction::StartHost,
    });
}

/// Starts a control-plane-only server as a background job.
pub fn confirm_start_serve(app: &mut App) {
    app.confirm = Some(Confirm {
        title: "Iniciar serve local".into(),
        lines: vec![
            "Sobe somente o control plane como job em segundo plano.".into(),
            "Usa os padrões (127.0.0.1:8080) e grava em serve.log.".into(),
            "Não inicia agente/TUN; pode conflitar com um host já ativo.".into(),
            "Para config/TLS, use o console: serve --help.".into(),
        ],
        action: ConfirmAction::StartServe,
    });
}

fn start_host(app: &mut App, tx: &Sender<Msg>) {
    if !is_root() {
        app.notify(Level::Error, "iniciar o host exige root");
        return;
    }
    launch_job(app, "host", vec!["host".into()], tx);
}

fn start_serve(app: &mut App, tx: &Sender<Msg>) {
    launch_job(app, "serve", vec!["serve".into()], tx);
}

pub fn confirm_install_service(app: &mut App) {
    app.confirm = Some(Confirm {
        title: "Instalar serviço do agente".into(),
        lines: vec![
            "Grava uma unit systemd ou launchd e inicia o agente no boot.".into(),
            format!(
                "Configuração usada: UDP {}, MTU {}, relay={}, expose={}",
                app.settings.udp_port,
                app.settings.mtu,
                if app.settings.force_relay { "sim" } else { "não" },
                if app.settings.expose.trim().is_empty() {
                    "—"
                } else {
                    app.settings.expose.trim()
                }
            ),
            "Exige root e pare o agente antes (o lock do data-dir bloqueia).".into(),
        ],
        action: ConfirmAction::InstallService,
    });
}

pub fn confirm_uninstall_service(app: &mut App) {
    app.confirm = Some(Confirm {
        title: "Remover serviço do agente".into(),
        lines: vec![
            "Desabilita e remove a unit do serviço (systemd/launchd).".into(),
            "O agente pode continuar rodando fora do serviço.".into(),
        ],
        action: ConfirmAction::UninstallService,
    });
}

// ---------------------------------------------------------------------------
// Peers and shares
// ---------------------------------------------------------------------------

pub fn ping_peer(app: &mut App, tx: &Sender<Msg>) {
    let Some(peer) = app.selected_peer().map(|peer| peer.name.clone()) else {
        app.notify(Level::Warn, "nenhum peer selecionado");
        return;
    };
    let socket = app.socket.clone();
    app.notify(Level::Info, format!("ping {peer}…"));
    let tx = tx.clone();
    thread::spawn(move || {
        let result = local_call(&socket, "ping", json!({ "peer": peer })).map(|value| {
            let rtt = value
                .get("rtt_ms_avg")
                .and_then(Value::as_f64)
                .map(|rtt| format!("{rtt:.1} ms"))
                .unwrap_or_else(|| "sem RTT".into());
            let received = value.get("received").and_then(Value::as_u64);
            let sent = value.get("sent").and_then(Value::as_u64);
            match (received, sent) {
                (Some(received), Some(sent)) => format!("{received}/{sent} respostas · {rtt}"),
                _ => rtt,
            }
        });
        let text = match result {
            Ok(detail) => format!("ping {peer}: {detail}"),
            Err(err) => format!("ping {peer} falhou: {err}"),
        };
        let _ = tx.send(Msg::Notify(
            if text.contains("falhou") {
                Level::Error
            } else {
                Level::Good
            },
            text,
        ));
    });
}

pub fn probe_peer(app: &mut App, peer: String, port: u16, tx: &Sender<Msg>) {
    let socket = app.socket.clone();
    app.notify(Level::Info, format!("probe {peer}:{port}…"));
    let tx = tx.clone();
    thread::spawn(move || {
        let result = local_call(&socket, "probe", json!({ "peer": peer, "port": port }));
        let (level, text) = match result {
            Ok(value) => {
                let ok = value
                    .get("connected")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let elapsed = value
                    .get("elapsed_ms")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                if ok {
                    (
                        Level::Good,
                        format!("{peer}:{port} conectado em {elapsed:.0} ms"),
                    )
                } else {
                    let error = value
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("sem conexão");
                    (Level::Error, format!("{peer}:{port} falhou: {error}"))
                }
            }
            Err(err) => (Level::Error, format!("probe {peer}:{port} falhou: {err}")),
        };
        let _ = tx.send(Msg::Notify(level, text));
    });
}

pub fn open_probe_form(app: &mut App) {
    if app.selected_peer().is_none() {
        app.notify(Level::Warn, "selecione um peer na aba Peers");
        return;
    }
    app.form = Some(crate::app::probe_form());
}

pub fn reconnect_peer(app: &mut App, tx: &Sender<Msg>) {
    let Some(peer) = app.selected_peer().map(|peer| peer.name.clone()) else {
        app.notify(Level::Warn, "nenhum peer selecionado");
        return;
    };
    let socket = app.socket.clone();
    app.notify(Level::Info, format!("reconectando {peer}…"));
    let tx = tx.clone();
    thread::spawn(move || {
        match local_call(&socket, "reconnect", json!({ "peer": peer })) {
            Ok(_) => {
                let _ = tx.send(Msg::Notify(
                    Level::Good,
                    format!("{peer}: reconexão disparada"),
                ));
            }
            Err(err) => {
                let _ = tx.send(Msg::Notify(
                    Level::Error,
                    format!("reconectar {peer}: {err}"),
                ));
            }
        }
    });
}

pub fn toggle_share(app: &mut App, tx: &Sender<Msg>) {
    let Some(share) = app.selected_share().cloned() else {
        app.notify(Level::Warn, "nenhum share selecionado");
        return;
    };
    if !app.is_own_share(&share) {
        app.notify(Level::Warn, "só o dono pode ligar/desligar este share");
        return;
    }
    let socket = app.socket.clone();
    let enabled = !share.enabled;
    let name = share.name.clone();
    let id = share.id;
    app.notify(
        Level::Info,
        format!("{} share {name}…", if enabled { "ativando" } else { "desativando" }),
    );
    let tx = tx.clone();
    thread::spawn(move || {
        match local_call(
            &socket,
            "share_update",
            json!({ "id": id, "enabled": enabled }),
        ) {
            Ok(_) => {
                let _ = tx.send(Msg::Notify(
                    Level::Good,
                    format!("share {name}: {}", if enabled { "ativo" } else { "desligado" }),
                ));
            }
            Err(err) => {
                let _ = tx.send(Msg::Notify(
                    Level::Error,
                    format!("share {name}: {err}"),
                ));
            }
        }
    });
}

pub fn request_delete_share(app: &mut App, tx: &Sender<Msg>) {
    let Some(share) = app.selected_share().cloned() else {
        app.notify(Level::Warn, "nenhum share selecionado");
        return;
    };
    if !app.is_own_share(&share) {
        app.notify(Level::Warn, "só o dono pode remover este share");
        return;
    }
    let armed = app
        .delete_armed
        .as_ref()
        .map(|(id, at)| *id == share.id && at.elapsed() < std::time::Duration::from_secs(4))
        .unwrap_or(false);
    if !armed {
        app.delete_armed = Some((share.id, Instant::now()));
        app.notify(
            Level::Warn,
            format!("pressione d de novo para remover '{}'", share.name),
        );
        return;
    }
    app.delete_armed = None;
    let id = share.id;
    let name = share.name.clone();
    delete_share(app, id, name, tx);
}

/// Opens the confirmation used by the mouse "Remover" action.
pub fn confirm_delete_share(app: &mut App) {
    let Some(share) = app.selected_share().cloned() else {
        app.notify(Level::Warn, "nenhum share selecionado");
        return;
    };
    if !app.is_own_share(&share) {
        app.notify(Level::Warn, "só o dono pode remover este share");
        return;
    }
    app.confirm = Some(Confirm {
        title: "Remover share".into(),
        lines: vec![
            format!("Remove '{}' do inventário da malha.", share.name),
            "O endereço deixa de aparecer para os outros nós.".into(),
        ],
        action: ConfirmAction::DeleteShare(share.id),
    });
}

fn delete_share(app: &mut App, id: Uuid, name: String, tx: &Sender<Msg>) {
    let socket = app.socket.clone();
    app.notify(Level::Info, format!("removendo share {name}…"));
    let tx = tx.clone();
    thread::spawn(move || {
        match local_call(&socket, "share_delete", json!({ "id": id })) {
            Ok(_) => {
                let _ = tx.send(Msg::Notify(
                    Level::Good,
                    format!("share {name} removido"),
                ));
            }
            Err(err) => {
                let _ = tx.send(Msg::Notify(
                    Level::Error,
                    format!("remover share {name}: {err}"),
                ));
            }
        }
    });
}

pub fn open_share(app: &mut App) {
    let Some(share) = app.selected_share().cloned() else {
        app.notify(Level::Warn, "nenhum share selecionado");
        return;
    };
    if !share.enabled {
        app.notify(Level::Warn, format!("share {} está desligado", share.name));
        return;
    }
    match share.kind {
        ShareKind::Web => {
            let url = share
                .public_url
                .clone()
                .unwrap_or_else(|| share.web_url());
            open_external(&url);
            app.notify(Level::Good, format!("abrindo {url}"));
        }
        ShareKind::Game => {
            let address = share.address();
            let copied = copy_text(&address);
            app.notify(
                Level::Info,
                format!(
                    "jogo {}: {} · steam://connect/{}{}",
                    share.name,
                    address,
                    address,
                    if copied { " · copiado" } else { "" }
                ),
            );
        }
        ShareKind::Tcp | ShareKind::Udp => {
            let address = share.address();
            let copied = copy_text(&address);
            app.notify(
                Level::Info,
                format!(
                    "{}: {}{}",
                    share.name,
                    address,
                    if copied { " · copiado" } else { "" }
                ),
            );
        }
    }
}

pub fn copy_selected_share(app: &mut App) {
    let Some(share) = app.selected_share().cloned() else {
        app.notify(Level::Warn, "nenhum share selecionado");
        return;
    };
    let address = share.address();
    if copy_text(&address) {
        app.notify(Level::Good, format!("{} copiado: {address}", share.name));
    } else {
        app.notify(Level::Warn, format!("não foi possível copiar; endereço: {address}"));
    }
}

pub fn copy_selected_peer(app: &mut App) {
    let Some(peer) = app.selected_peer().cloned() else {
        app.notify(Level::Warn, "nenhum peer selecionado");
        return;
    };
    let address = peer.overlay_ip.to_string();
    if copy_text(&address) {
        app.notify(Level::Good, format!("{} copiado: {address}", peer.name));
    } else {
        app.notify(Level::Warn, format!("não foi possível copiar: {address}"));
    }
}

pub fn open_external(url: &str) {
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(target_os = "linux")]
    let program = "xdg-open";
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let program = "";
    if program.is_empty() {
        return;
    }
    let _ = std::process::Command::new(program)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

pub fn copy_text(value: &str) -> bool {
    #[cfg(target_os = "macos")]
    let (program, args): (&str, &[&str]) = ("pbcopy", &[]);
    #[cfg(target_os = "linux")]
    let (program, args): (&str, &[&str]) = ("wl-copy", &[]);
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let (program, args): (&str, &[&str]) = ("", &[]);
    if program.is_empty() {
        return false;
    }
    use std::io::Write;
    let Ok(mut child) = std::process::Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    if let Some(stdin) = child.stdin.as_mut() {
        if stdin.write_all(value.as_bytes()).is_err() {
            let _ = child.kill();
            return false;
        }
    }
    child
        .wait()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_are_validated() {
        assert_eq!(parse_port("3000", "x").unwrap(), 3000);
        assert!(parse_port("0", "x").is_err());
        assert!(parse_port("abc", "x").is_err());
        assert!(parse_port("70000", "x").is_err());
    }
}
