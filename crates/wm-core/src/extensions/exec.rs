//! Generic `exec` driver: one supervised process per extension.

use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use wm_ext::{ExecConfig, RestartPolicy};
use wm_proto::ExtensionState;

use super::process::{ManagedProcess, ProcessSpec, clip_line, looks_like_error};
use super::Reporter;

pub(crate) async fn run(config: ExecConfig, reporter: Reporter, cancel: CancellationToken) {
    let mut backoff = Duration::from_secs(1);
    reporter.update(|status| {
        status.public_urls = config.public_urls.clone();
    });
    loop {
        if cancel.is_cancelled() {
            reporter.set(ExtensionState::Stopped, "parado");
            return;
        }
        reporter.set(ExtensionState::Starting, format!("iniciando {}", config.command));
        let mut spec = ProcessSpec::new(config.command.clone()).args(config.args.clone());
        for (key, value) in &config.env {
            spec = spec.env(key.clone(), value.clone());
        }
        if let Some(dir) = &config.working_dir {
            spec = spec.cwd(dir);
        }
        let process = match ManagedProcess::spawn_logged(&spec, Some(reporter.logs.clone()), "exec.") {
            Ok(process) => process,
            Err(err) => {
                reporter.set(
                    ExtensionState::Error,
                    format!("falha ao iniciar '{}': {err}", config.command),
                );
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = tokio::time::sleep(Duration::from_secs(10)) => {}
                }
                continue;
            }
        };
        let mut process = process;
        if let Some(pid) = process.pid() {
            reporter.update(|status| {
                status.state = ExtensionState::Running;
                status.message = Some(format!("pid {pid}"));
                status.details = serde_json::json!({ "pid": pid, "command": config.command });
            });
        } else {
            reporter.set(ExtensionState::Running, "processo iniciado");
        }

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
                            tracing::debug!(target: "wm::ext::exec", line = %super::logs::sanitize(&line), "sidecar");
                            if looks_like_error(&line) {
                                reporter.update(|status| {
                                    status.message = Some(clip_line(&line));
                                });
                            }
                        }
                        None => read_lines = false,
                    }
                }
            }
        }

        if cancelled {
            process.terminate().await;
            reporter.set(ExtensionState::Stopped, "parado pelo control plane");
            return;
        }

        process.finish_output().await;
        let exit = exit.unwrap_or_else(|| Err(std::io::Error::other("processo sem status")));
        let success = exit.as_ref().map(|status| status.success()).unwrap_or(false);
        let code = match &exit {
            Ok(status) => status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "sinal".into()),
            Err(err) => err.to_string(),
        };
        reporter.logs.record("lifecycle", &format!("exec exited: {code}"));
        let restart = match config.restart {
            RestartPolicy::Always => true,
            RestartPolicy::OnFailure => !success,
            RestartPolicy::Never => false,
        };
        if !restart {
            reporter.set(
                if success {
                    ExtensionState::Stopped
                } else {
                    ExtensionState::Error
                },
                format!("saiu ({code})"),
            );
            return;
        }
        reporter.logs.record("lifecycle", &format!("exec restarting in {}s", backoff.as_secs()));
        reporter.set(
            if success {
                ExtensionState::Stopped
            } else {
                ExtensionState::Error
            },
            format!("saiu ({code}); reiniciando em {}s", backoff.as_secs()),
        );
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
