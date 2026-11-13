//! Lifecycle hooks run by the agent on behalf of an extension.
//!
//! Hooks are declarative commands embedded in the manifest (trusted admin
//! content). They run directly, without a shell, with a `WM_*` environment and
//! a timeout. `install` is remembered per extension version; `activate` runs
//! before the sidecar and `deactivate` after it stops.
//!
//! Hook commands are serialized per extension by the supervisor, so an old
//! deactivate cannot race the next activate during a config change.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use chrono::Utc;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use wm_proto::{ExtensionDesired, ExtensionState, HookSpec};

use super::Reporter;

#[derive(Debug, Clone)]
pub struct HookContext {
    pub node_id: Uuid,
    pub node_name: String,
    pub overlay_ip: std::net::Ipv4Addr,
    pub control_url: String,
    pub data_dir: PathBuf,
    pub extensions_dir: PathBuf,
}

impl HookContext {
    fn extension_dir(&self, desired: &ExtensionDesired) -> PathBuf {
        self.extensions_dir.join(&desired.id)
    }
}

struct HookOutput {
    success: bool,
    summary: String,
}

/// Runs install (once) and activate. Returns an error string suitable for the
/// extension status when a hook fails.
pub async fn prepare(
    desired: &ExtensionDesired,
    ctx: &HookContext,
    reporter: &Reporter,
    cancel: &CancellationToken,
) -> Result<(), String> {
    let ext_dir = ctx.extension_dir(desired);
    if let Err(err) = tokio::fs::create_dir_all(&ext_dir).await {
        return Err(format!("não foi possível criar {}: {err}", ext_dir.display()));
    }
    write_config_file(&ext_dir, desired)?;

    if let Some(hook) = &desired.hooks.install {
        let marker = install_marker(&ext_dir, desired);
        if !marker.exists() {
            reporter.set(ExtensionState::Starting, format!("hook install ({})", desired.id));
            let output = run_hook("install", hook, desired, ctx, cancel).await?;
            if !output.success {
                return Err(format!("hook install falhou: {}", output.summary));
            }
            if let Err(err) = std::fs::create_dir_all(marker.parent().unwrap_or(&ext_dir)) {
                tracing::warn!(%err, "hook install: não foi possível criar diretório do marcador");
            }
            if let Err(err) = std::fs::write(&marker, Utc::now().to_rfc3339()) {
                tracing::warn!(%err, "hook install: não foi possível gravar marcador");
            }
        }
    }

    if let Some(hook) = &desired.hooks.activate {
        reporter.set(ExtensionState::Starting, format!("hook activate ({})", desired.id));
        let output = run_hook("activate", hook, desired, ctx, cancel).await?;
        if !output.success {
            return Err(format!("hook activate falhou: {}", output.summary));
        }
    }
    Ok(())
}

/// Best-effort cleanup. Errors are logged, not reported: the extension is
/// already leaving the desired set, so there is no status row to attach them.
pub async fn deactivate(desired: &ExtensionDesired, ctx: &HookContext) {
    let Some(hook) = &desired.hooks.deactivate else {
        return;
    };
    let cancel = CancellationToken::new();
    match run_hook("deactivate", hook, desired, ctx, &cancel).await {
        Ok(output) if output.success => {}
        Ok(output) => tracing::warn!(
            extension = %desired.id,
            summary = %output.summary,
            "hook deactivate falhou"
        ),
        Err(err) => tracing::warn!(extension = %desired.id, %err, "hook deactivate falhou"),
    }
}

fn write_config_file(ext_dir: &Path, desired: &ExtensionDesired) -> Result<(), String> {
    let path = ext_dir.join("config.json");
    let text = serde_json::to_vec_pretty(&desired.config)
        .map_err(|err| format!("config não serializável: {err}"))?;
    std::fs::write(&path, text).map_err(|err| format!("gravando {}: {err}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn install_marker(ext_dir: &Path, desired: &ExtensionDesired) -> PathBuf {
    let version: String = desired
        .version
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .collect();
    let version = if version.is_empty() { "0".to_string() } else { version };
    ext_dir.join("hooks").join(format!("install-{version}.ok"))
}

async fn run_hook(
    name: &str,
    hook: &HookSpec,
    desired: &ExtensionDesired,
    ctx: &HookContext,
    cancel: &CancellationToken,
) -> Result<HookOutput, String> {
    let ext_dir = ctx.extension_dir(desired);
    let command = expand(&hook.command, name, desired, ctx);
    let args: Vec<String> = hook
        .args
        .iter()
        .map(|arg| expand(arg, name, desired, ctx))
        .collect();

    let mut env: BTreeMap<String, String> = BTreeMap::new();
    env.insert("WM_NODE_ID".into(), ctx.node_id.to_string());
    env.insert("WM_NODE_NAME".into(), ctx.node_name.clone());
    env.insert("WM_OVERLAY_IP".into(), ctx.overlay_ip.to_string());
    env.insert("WM_CONTROL_URL".into(), ctx.control_url.clone());
    env.insert("WM_DATA_DIR".into(), ctx.data_dir.display().to_string());
    env.insert("WM_EXTENSION_ID".into(), desired.id.clone());
    env.insert("WM_EXTENSION_NAME".into(), desired.name.clone());
    env.insert("WM_EXTENSION_VERSION".into(), desired.version.clone());
    env.insert("WM_EXTENSION_DRIVER".into(), desired.driver.clone());
    env.insert("WM_EXTENSION_CONFIG".into(), desired.config.to_string());
    env.insert("WM_EXTENSION_CONFIG_FILE".into(), ext_dir.join("config.json").display().to_string());
    env.insert("WM_EXTENSION_DIR".into(), ext_dir.display().to_string());
    env.insert("WM_HOOK".into(), name.into());
    for (key, value) in &hook.env {
        env.insert(key.clone(), expand(value, name, desired, ctx));
    }

    let working_dir = hook
        .working_dir
        .as_deref()
        .map(|dir| PathBuf::from(expand(dir, name, desired, ctx)))
        .unwrap_or_else(|| ext_dir.clone());
    if let Err(err) = tokio::fs::create_dir_all(&working_dir).await {
        return Err(format!("criando {}: {err}", working_dir.display()));
    }

    tracing::info!(
        extension = %desired.id,
        hook = name,
        command = %command,
        "executando hook"
    );

    let mut child = Command::new(&command)
        .args(&args)
        .envs(&env)
        .current_dir(&working_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| format!("falha ao executar '{command}': {err}"))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_task = stdout.map(|mut pipe| {
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf).await;
            String::from_utf8_lossy(&buf).into_owned()
        })
    });
    let stderr_task = stderr.map(|mut pipe| {
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf).await;
            String::from_utf8_lossy(&buf).into_owned()
        })
    });

    let timeout = Duration::from_secs(hook.timeout_secs.clamp(1, 3600));
    let status = tokio::select! {
        _ = cancel.cancelled() => {
            let _ = child.start_kill();
            return Err("cancelado".into());
        }
        result = tokio::time::timeout(timeout, child.wait()) => {
            match result {
                Ok(Ok(status)) => status,
                Ok(Err(err)) => return Err(format!("hook {name} falhou ao esperar: {err}")),
                Err(_) => {
                    let _ = child.start_kill();
                    return Err(format!("hook {name} excedeu {}s", timeout.as_secs()));
                }
            }
        }
    };

    let stdout = join_output(stdout_task).await;
    let stderr = join_output(stderr_task).await;
    let combined = if stderr.trim().is_empty() { stdout } else { stderr };
    let summary = summarize(&combined);
    if !summary.is_empty() {
        tracing::info!(extension = %desired.id, hook = name, success = status.success(), %summary, "hook terminou");
    }
    Ok(HookOutput {
        success: status.success(),
        summary: if summary.is_empty() {
            format!("exit {}", status.code().unwrap_or(-1))
        } else {
            summary
        },
    })
}

async fn join_output(task: Option<tokio::task::JoinHandle<String>>) -> String {
    match task {
        Some(task) => tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default(),
        None => String::new(),
    }
}

fn summarize(output: &str) -> String {
    let line = output
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    // Hook output is shown in the TUI/panel. Strip control characters so an
    // ANSI escape or terminal control sequence cannot reach the renderer.
    let cleaned: String = line.chars().filter(|ch| !ch.is_control()).collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut summary: String = cleaned.chars().take(300).collect();
    if cleaned.chars().count() > 300 {
        summary.push('…');
    }
    summary
}

fn expand(value: &str, hook: &str, desired: &ExtensionDesired, ctx: &HookContext) -> String {
    let ext_dir = ctx.extension_dir(desired);
    value
        .replace("{node_id}", &ctx.node_id.to_string())
        .replace("{node_name}", &ctx.node_name)
        .replace("{overlay_ip}", &ctx.overlay_ip.to_string())
        .replace("{control_url}", &ctx.control_url)
        .replace("{data_dir}", &ctx.data_dir.display().to_string())
        .replace("{extension_id}", &desired.id)
        .replace("{extension_version}", &desired.version)
        .replace("{extension_dir}", &ext_dir.display().to_string())
        .replace("{hook}", hook)
}
