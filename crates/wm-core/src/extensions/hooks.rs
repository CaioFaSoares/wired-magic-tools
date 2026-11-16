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
use std::time::Duration;

use chrono::Utc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use wm_proto::{ExtensionDesired, ExtensionState, HookSpec};

use super::Reporter;
use super::process::{ManagedProcess, ProcessSpec};

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
            let output = run_hook("install", hook, desired, ctx, cancel, reporter).await?;
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
        let output = run_hook("activate", hook, desired, ctx, cancel, reporter).await?;
        if !output.success {
            return Err(format!("hook activate falhou: {}", output.summary));
        }
    }
    Ok(())
}

/// Best-effort cleanup. Errors are logged, not reported: the extension is
/// already leaving the desired set, so there is no status row to attach them.
pub async fn deactivate(desired: &ExtensionDesired, ctx: &HookContext, reporter: &Reporter) {
    let Some(hook) = &desired.hooks.deactivate else {
        return;
    };
    let cancel = CancellationToken::new();
    match run_hook("deactivate", hook, desired, ctx, &cancel, reporter).await {
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
    reporter: &Reporter,
) -> Result<HookOutput, String> {
    let ext_dir = ctx.extension_dir(desired);
    let command = expand(&hook.command, name, desired, ctx);
    let args: Vec<String> = hook
        .args
        .iter()
        .map(|arg| expand(arg, name, desired, ctx))
        .collect();

    let mut env: BTreeMap<String, String> = BTreeMap::new();
    if let Ok(binary) = std::env::current_exe() {
        env.insert("WM_AGENT_BIN".into(), binary.display().to_string());
    }
    env.insert("WM_NODE_ID".into(), ctx.node_id.to_string());
    env.insert("WM_NODE_NAME".into(), ctx.node_name.clone());
    env.insert("WM_OVERLAY_IP".into(), ctx.overlay_ip.to_string());
    env.insert("WM_CONTROL_URL".into(), ctx.control_url.clone());
    env.insert("WM_DATA_DIR".into(), ctx.data_dir.display().to_string());
    env.insert("WM_API_SOCKET".into(), ctx.data_dir.join("agent.sock").display().to_string());
    env.insert("WM_API_VERSION".into(), wm_ext::CONTEXT_API_VERSION.to_string());
    env.insert("WM_CONTEXT_METHOD".into(), wm_ext::CONTEXT_METHOD.into());
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

    reporter.logs.record("lifecycle", &format!("hook {name} starting"));
    // Never log command, args, env or config. The common reader sanitizes and
    // bounds both pipes, and its tasks are owned by ManagedProcess on all exits.
    let spec = ProcessSpec { program: command, args, env, cwd: Some(working_dir) };
    let mut process = ManagedProcess::spawn_logged(&spec, Some(reporter.logs.clone()), &format!("hook.{name}."))
        .map_err(|_| format!("hook {name}: falha ao iniciar processo"))?;
    let timeout = Duration::from_secs(hook.timeout_secs.clamp(1, 3600));
    let result = tokio::select! {
        _ = cancel.cancelled() => Err(format!("hook {name} cancelado")),
        result = tokio::time::timeout(timeout, process.child.wait()) => {
            match result {
                Ok(Ok(status)) => Ok(status),
                Ok(Err(_)) => Err(format!("hook {name} falhou ao esperar")),
                Err(_) => Err(format!("hook {name} excedeu {}s", timeout.as_secs())),
            }
        }
    };
    let status = match result {
        Ok(status) => status,
        Err(err) => {
            process.terminate().await;
            reporter.logs.record("lifecycle", &err);
            return Err(err);
        }
    };
    process.finish_output().await;
    let summary = format!("exit {}", status.code().unwrap_or(-1));
    reporter.logs.record("lifecycle", &format!("hook {name} finished: {summary}"));
    Ok(HookOutput { success: status.success(), summary })
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
