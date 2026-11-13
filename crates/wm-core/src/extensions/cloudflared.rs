//! Built-in cloudflared node driver.
//!
//! One task supervises all routes of the extension:
//!
//! * one `cloudflared tunnel --url` process per quick route;
//! * one `cloudflared tunnel --config ... run` process for all named routes.
//!
//! Each process is restarted with exponential backoff. Quick tunnel URLs are
//! parsed from the process log and reported back to the control plane.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use wm_ext::{
    CloudflaredConfig, CloudflaredRouteConfig, CloudflaredTunnelConfig,
    extract_quick_tunnel_url, render_named_config, validate_binary, validate_local_url,
};
use wm_proto::ExtensionState;

use super::process::{ManagedProcess, ProcessSpec, clip_line, looks_like_error};
use super::Reporter;

#[derive(Debug, Clone, Serialize)]
struct RouteLive {
    id: String,
    project: String,
    state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    public_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

#[derive(Debug, Default)]
struct Live {
    routes: BTreeMap<String, RouteLive>,
}

pub(crate) async fn run(
    config: CloudflaredConfig,
    reporter: Reporter,
    cancel: CancellationToken,
    work_dir: PathBuf,
) {
    let binary = match validate_binary(&config.binary) {
        Ok(binary) => binary,
        Err(err) => {
            reporter.set(ExtensionState::Error, err);
            cancel.cancelled().await;
            return;
        }
    };
    if let Err(err) = std::fs::create_dir_all(&work_dir) {
        reporter.set(
            ExtensionState::Error,
            format!("não foi possível criar {}: {err}", work_dir.display()),
        );
        cancel.cancelled().await;
        return;
    }

    let live = Arc::new(Mutex::new(Live::default()));
    {
        let mut state = live.lock().expect("live status");
        for route in &config.quick {
            state.routes.insert(
                route.id.clone(),
                RouteLive {
                    id: route.id.clone(),
                    project: route.project.clone(),
                    state: "starting".into(),
                    public_url: None,
                    message: None,
                },
            );
        }
        for route in &config.named {
            state.routes.insert(
                route.id.clone(),
                RouteLive {
                    id: route.id.clone(),
                    project: route.project.clone(),
                    state: "starting".into(),
                    public_url: None,
                    message: None,
                },
            );
        }
    }

    let (notify_tx, mut notify_rx) = mpsc::channel::<()>(128);
    let child_cancel = CancellationToken::new();
    let mut join = JoinSet::new();

    for route in config.quick.clone() {
        let binary = binary.clone();
        let live = live.clone();
        let notify = notify_tx.clone();
        let cancel = child_cancel.clone();
        join.spawn(async move {
            run_quick(route, binary, live, notify, cancel).await;
        });
    }

    if !config.named.is_empty() {
        match &config.tunnel {
            Some(tunnel) => match prepare_named(&work_dir, tunnel, &config.named) {
                Ok(config_path) => {
                    let binary = binary.clone();
                    let live = live.clone();
                    let notify = notify_tx.clone();
                    let cancel = child_cancel.clone();
                    let routes = config.named.clone();
                    join.spawn(async move {
                        run_named(routes, binary, config_path, live, notify, cancel).await;
                    });
                }
                Err(err) => {
                    for route in &config.named {
                        set_route(&live, &route.id, "error", None, Some(err.clone()));
                    }
                    let _ = notify_tx.try_send(());
                }
            },
            None => {
                let message = "rotas named sem túnel configurado neste nó".to_string();
                for route in &config.named {
                    set_route(&live, &route.id, "error", None, Some(message.clone()));
                }
                let _ = notify_tx.try_send(());
            }
        }
    }

    publish(&reporter, &live);
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                child_cancel.cancel();
                break;
            }
            Some(_) = notify_rx.recv() => publish(&reporter, &live),
            _ = tick.tick() => publish(&reporter, &live),
            Some(result) = join.join_next(), if !join.is_empty() => {
                if let Err(err) = result {
                    tracing::warn!(%err, "tarefa do cloudflared terminou com erro");
                    notify_tx.try_send(()).ok();
                }
            }
        }
    }
    // Give children a moment to kill their processes; JoinSet aborts the rest.
    let _ = tokio::time::timeout(Duration::from_secs(3), async {
        while join.join_next().await.is_some() {}
    })
    .await;
    reporter.set(ExtensionState::Stopped, "parado pelo control plane");
}

fn prepare_named(
    work_dir: &Path,
    tunnel: &CloudflaredTunnelConfig,
    routes: &[CloudflaredRouteConfig],
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(work_dir).map_err(|err| format!("criando diretório: {err}"))?;
    let credentials_path = work_dir.join("credentials.json");
    write_secret(&credentials_path, tunnel.credentials_json.as_bytes())
        .map_err(|err| format!("gravando credenciais: {err}"))?;
    let config = render_named_config(&tunnel.tunnel_id, &credentials_path, routes)?;
    let config_path = work_dir.join("config.yml");
    std::fs::write(&config_path, config).map_err(|err| format!("gravando config: {err}"))?;
    Ok(config_path)
}

fn write_secret(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
    }
    file.write_all(bytes)?;
    file.flush()
}

async fn run_quick(
    route: CloudflaredRouteConfig,
    binary: String,
    live: Arc<Mutex<Live>>,
    notify: mpsc::Sender<()>,
    cancel: CancellationToken,
) {
    let local_url = match validate_local_url(&route.local_url) {
        Ok(url) => url,
        Err(err) => {
            set_route(&live, &route.id, "error", None, Some(err));
            notify.try_send(()).ok();
            cancel.cancelled().await;
            return;
        }
    };
    let mut backoff = Duration::from_secs(1);
    loop {
        if cancel.is_cancelled() {
            set_route(&live, &route.id, "stopped", None, Some("parada".into()));
            notify.try_send(()).ok();
            return;
        }
        set_route(
            &live,
            &route.id,
            "starting",
            None,
            Some(format!("expondo {local_url}")),
        );
        notify.try_send(()).ok();
        let spec = ProcessSpec::new(binary.clone()).args([
            "tunnel",
            "--no-autoupdate",
            "--url",
            local_url.as_str(),
        ]);
        let mut process = match ManagedProcess::spawn(&spec) {
            Ok(process) => process,
            Err(err) => {
                set_route(
                    &live,
                    &route.id,
                    "error",
                    None,
                    Some(format!(
                        "falha ao executar '{binary}': {err}; instale o cloudflared ou ajuste o binário do nó"
                    )),
                );
                notify.try_send(()).ok();
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = tokio::time::sleep(Duration::from_secs(10)) => {}
                }
                continue;
            }
        };
        let started = Instant::now();
        let mut read_lines = true;
        let mut cancelled = false;
        let mut exit: Option<Result<std::process::ExitStatus, std::io::Error>> = None;
        loop {
            let child = &mut process.child;
            let lines = &mut process.lines;
            tokio::select! {
                _ = cancel.cancelled() => {
                    cancelled = true;
                    break;
                }
                status = child.wait() => {
                    exit = Some(status);
                    break;
                }
                line = async {
                    if read_lines {
                        lines.recv().await
                    } else {
                        std::future::pending::<Option<String>>().await
                    }
                } => {
                    match line {
                        Some(line) => {
                            tracing::debug!(target: "wm::ext::cloudflared", project = %route.project, %line, "quick tunnel");
                            if let Some(url) = extract_quick_tunnel_url(&line) {
                                set_route(
                                    &live,
                                    &route.id,
                                    "running",
                                    Some(url),
                                    Some("túnel rápido ativo".into()),
                                );
                                notify.try_send(()).ok();
                            } else if looks_like_error(&line) {
                                set_route(
                                    &live,
                                    &route.id,
                                    "error",
                                    None,
                                    Some(clip_line(&line)),
                                );
                                notify.try_send(()).ok();
                            }
                        }
                        None => read_lines = false,
                    }
                }
            }
        }
        if cancelled {
            process.terminate().await;
            set_route(&live, &route.id, "stopped", None, Some("parada".into()));
            notify.try_send(()).ok();
            return;
        }
        let exit = exit.unwrap_or_else(|| Err(std::io::Error::other("processo sem status")));
        let code = match &exit {
            Ok(status) => status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "sinal".into()),
            Err(err) => err.to_string(),
        };
        set_route(
            &live,
            &route.id,
            "error",
            None,
            Some(format!("cloudflared saiu ({code}); reiniciando em {}s", backoff.as_secs())),
        );
        notify.try_send(()).ok();
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(backoff) => {}
        }
        backoff = if started.elapsed() > Duration::from_secs(60) {
            Duration::from_secs(1)
        } else {
            (backoff * 2).min(Duration::from_secs(30))
        };
    }
}

async fn run_named(
    routes: Vec<CloudflaredRouteConfig>,
    binary: String,
    config_path: PathBuf,
    live: Arc<Mutex<Live>>,
    notify: mpsc::Sender<()>,
    cancel: CancellationToken,
) {
    let mut backoff = Duration::from_secs(1);
    loop {
        if cancel.is_cancelled() {
            for route in &routes {
                set_route(&live, &route.id, "stopped", None, Some("parada".into()));
            }
            notify.try_send(()).ok();
            return;
        }
        for route in &routes {
            set_route(
                &live,
                &route.id,
                "starting",
                None,
                Some("conectando ao túnel named".into()),
            );
        }
        notify.try_send(()).ok();
        let spec = ProcessSpec::new(binary.clone()).args([
            "tunnel",
            "--no-autoupdate",
            "--config",
            config_path.to_string_lossy().as_ref(),
            "run",
        ]);
        let mut process = match ManagedProcess::spawn(&spec) {
            Ok(process) => process,
            Err(err) => {
                for route in &routes {
                    set_route(
                        &live,
                        &route.id,
                        "error",
                        None,
                        Some(format!(
                            "falha ao executar '{binary}': {err}; instale o cloudflared ou ajuste o binário do nó"
                        )),
                    );
                }
                notify.try_send(()).ok();
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = tokio::time::sleep(Duration::from_secs(10)) => {}
                }
                continue;
            }
        };
        let started = Instant::now();
        let mut read_lines = true;
        let mut cancelled = false;
        let mut exit: Option<Result<std::process::ExitStatus, std::io::Error>> = None;
        loop {
            let child = &mut process.child;
            let lines = &mut process.lines;
            tokio::select! {
                _ = cancel.cancelled() => {
                    cancelled = true;
                    break;
                }
                status = child.wait() => {
                    exit = Some(status);
                    break;
                }
                line = async {
                    if read_lines {
                        lines.recv().await
                    } else {
                        std::future::pending::<Option<String>>().await
                    }
                } => {
                    match line {
                        Some(line) => {
                            tracing::debug!(target: "wm::ext::cloudflared", %line, "named tunnel");
                            if line.to_ascii_lowercase().contains("registered tunnel connection") {
                                for route in &routes {
                                    let hostname = route.hostname.clone().unwrap_or_default();
                                    set_route(
                                        &live,
                                        &route.id,
                                        "running",
                                        Some(format!("https://{hostname}")),
                                        Some("túnel named conectado".into()),
                                    );
                                }
                                notify.try_send(()).ok();
                            } else if looks_like_error(&line) {
                                for route in &routes {
                                    set_route(
                                        &live,
                                        &route.id,
                                        "error",
                                        None,
                                        Some(clip_line(&line)),
                                    );
                                }
                                notify.try_send(()).ok();
                            }
                        }
                        None => read_lines = false,
                    }
                }
            }
        }
        if cancelled {
            process.terminate().await;
            for route in &routes {
                set_route(&live, &route.id, "stopped", None, Some("parada".into()));
            }
            notify.try_send(()).ok();
            return;
        }
        let exit = exit.unwrap_or_else(|| Err(std::io::Error::other("processo sem status")));
        let code = match &exit {
            Ok(status) => status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "sinal".into()),
            Err(err) => err.to_string(),
        };
        for route in &routes {
            set_route(
                &live,
                &route.id,
                "error",
                None,
                Some(format!("cloudflared saiu ({code}); reiniciando em {}s", backoff.as_secs())),
            );
        }
        notify.try_send(()).ok();
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(backoff) => {}
        }
        backoff = if started.elapsed() > Duration::from_secs(60) {
            Duration::from_secs(1)
        } else {
            (backoff * 2).min(Duration::from_secs(30))
        };
    }
}

fn set_route(
    live: &Arc<Mutex<Live>>,
    id: &str,
    state: &str,
    public_url: Option<String>,
    message: Option<String>,
) {
    if let Ok(mut live) = live.lock() {
        if let Some(route) = live.routes.get_mut(id) {
            route.state = state.to_string();
            route.public_url = public_url;
            route.message = message;
        }
    }
}

fn publish(reporter: &Reporter, live: &Arc<Mutex<Live>>) {
    let routes: Vec<RouteLive> = live
        .lock()
        .map(|live| live.routes.values().cloned().collect())
        .unwrap_or_default();
    let running = routes.iter().filter(|r| r.state == "running").count();
    let starting = routes.iter().filter(|r| r.state == "starting").count();
    let errors = routes.iter().filter(|r| r.state == "error").count();
    let state = if running > 0 {
        ExtensionState::Running
    } else if starting > 0 {
        ExtensionState::Starting
    } else if errors > 0 {
        ExtensionState::Error
    } else {
        ExtensionState::Stopped
    };
    let first_error = routes
        .iter()
        .find(|r| r.state == "error")
        .and_then(|r| r.message.clone());
    let message = if let Some(error) = first_error {
        if running > 0 {
            format!("{running} rota(s) ativa(s); erro em outra: {error}")
        } else {
            error
        }
    } else if routes.is_empty() {
        "nenhuma rota configurada".into()
    } else if running > 0 {
        format!("{running}/{} rota(s) ativa(s)", routes.len())
    } else if starting > 0 {
        "iniciando rotas".into()
    } else {
        "sem rotas ativas".into()
    };
    let public_urls: Vec<String> = routes
        .iter()
        .filter(|r| r.state == "running")
        .filter_map(|r| r.public_url.clone())
        .collect();
    let details = serde_json::json!({ "routes": routes });
    reporter.set_routes(state, Some(message), public_urls, details);
}
