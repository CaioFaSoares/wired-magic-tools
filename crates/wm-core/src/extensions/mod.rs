//! Node-side extension supervisor.
//!
//! The control plane sends an authoritative `Vec<ExtensionDesired>` in every
//! map. This module reconciles that list into a set of supervised sidecars:
//!
//! * an extension absent from the list is stopped;
//! * a changed `revision`, `config` or `enabled` restarts it;
//! * an unchanged extension is left alone, even when the map version changes
//!   for unrelated reasons (peer endpoints, ACL, ...).
//!
//! Observed state is published through a `watch` channel. The agent's report
//! loop forwards it to the control plane; the local API exposes it to `wm`.
//!
//! Drivers available today: `cloudflared` and `exec`. A manifest with
//! `node_driver = "none"` never reaches this module.

pub(crate) mod cloudflared;
pub(crate) mod exec;
pub(crate) mod hooks;
mod process;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use tokio::sync::{Mutex as TokioMutex, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use wm_proto::{ExtensionDesired, ExtensionState, ExtensionStatus};

use self::hooks::HookContext;

/// Shared status cell for one extension instance.
#[derive(Clone)]
pub(crate) struct Reporter {
    inner: Arc<Mutex<ExtensionStatus>>,
}

impl Reporter {
    fn new(desired: &ExtensionDesired) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ExtensionStatus {
                id: desired.id.clone(),
                name: desired.name.clone(),
                driver: desired.driver.clone(),
                state: ExtensionState::Starting,
                message: None,
                public_urls: Vec::new(),
                details: serde_json::Value::Null,
                updated_at: Utc::now(),
            })),
        }
    }

    pub(crate) fn update(&self, f: impl FnOnce(&mut ExtensionStatus)) {
        if let Ok(mut status) = self.inner.lock() {
            f(&mut status);
            status.updated_at = Utc::now();
        }
    }

    pub(crate) fn set(&self, state: ExtensionState, message: impl Into<String>) {
        let message = message.into();
        if let Ok(mut status) = self.inner.lock() {
            if status.state == state && status.message.as_deref() == Some(message.as_str()) {
                return;
            }
            status.state = state;
            status.message = Some(message);
            status.updated_at = Utc::now();
        }
    }

    /// Publishes a full snapshot. No-op when nothing changed: the aggregator
    /// calls this every second, and an unconditional timestamp update would
    /// wake the report loop (and the panel) forever.
    pub(crate) fn set_routes(
        &self,
        state: ExtensionState,
        message: Option<String>,
        public_urls: Vec<String>,
        details: serde_json::Value,
    ) {
        if let Ok(mut status) = self.inner.lock() {
            if status.state == state
                && status.message == message
                && status.public_urls == public_urls
                && status.details == details
            {
                return;
            }
            status.state = state;
            status.message = message;
            status.public_urls = public_urls;
            status.details = details;
            status.updated_at = Utc::now();
        }
    }

    fn snapshot(&self) -> ExtensionStatus {
        self.inner
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|_| ExtensionStatus {
                id: "?".into(),
                name: "?".into(),
                driver: "?".into(),
                state: ExtensionState::Error,
                message: Some("status envenenado".into()),
                public_urls: Vec::new(),
                details: serde_json::Value::Null,
                updated_at: Utc::now(),
            })
    }
}

struct Handle {
    revision: u64,
    activation_revision: u64,
    enabled: bool,
    fingerprint: String,
    reporter: Reporter,
    cancel: CancellationToken,
    _task: JoinHandle<()>,
}

impl Drop for Handle {
    fn drop(&mut self) {
        // Do not abort: the task still has to run its deactivate hook after the
        // driver notices the cancellation. The per-extension lock guarantees
        // the next activate waits for this cleanup.
        self.cancel.cancel();
    }
}

/// Long-running reconciliation loop. It never returns on its own; the agent
/// aborts it on shutdown, and dropping the handles terminates the sidecars.
pub async fn run(
    mut desired_rx: watch::Receiver<Vec<ExtensionDesired>>,
    status_tx: watch::Sender<Vec<ExtensionStatus>>,
    work_dir: PathBuf,
    ctx: HookContext,
) {
    let mut handles: HashMap<String, Handle> = HashMap::new();
    let mut locks: HashMap<String, Arc<TokioMutex<()>>> = HashMap::new();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // The map may already contain desired state when the agent starts (for
    // example after a reconnect). Process the current value before waiting.
    let initial = desired_rx.borrow_and_update().clone();
    reconcile(&mut handles, &mut locks, initial, &work_dir, &ctx);
    publish(&status_tx, &handles);
    loop {
        tokio::select! {
            changed = desired_rx.changed() => {
                if changed.is_err() {
                    break;
                }
                let desired = desired_rx.borrow_and_update().clone();
                reconcile(&mut handles, &mut locks, desired, &work_dir, &ctx);
                publish(&status_tx, &handles);
            }
            _ = tick.tick() => {
                publish(&status_tx, &handles);
            }
        }
    }
    handles.clear();
    publish(&status_tx, &handles);
}

fn reconcile(
    handles: &mut HashMap<String, Handle>,
    locks: &mut HashMap<String, Arc<TokioMutex<()>>>,
    desired: Vec<ExtensionDesired>,
    work_dir: &std::path::Path,
    ctx: &HookContext,
) {
    let keep: std::collections::HashSet<String> =
        desired.iter().map(|d| d.id.clone()).collect();
    handles.retain(|id, _| keep.contains(id));

    for desired in desired {
        // A hook, driver or version change must also restart the instance so
        // the new install/activate code actually runs. Config alone is not
        // enough after a manifest reload.
        let fingerprint = serde_json::to_string(&(
            &desired.config,
            &desired.hooks,
            &desired.version,
            &desired.driver,
        ))
        .unwrap_or_default();
        let unchanged = handles
            .get(&desired.id)
            .map(|handle| {
                handle.revision == desired.revision
                    && handle.activation_revision == desired.activation_revision
                    && handle.enabled == desired.enabled
                    && handle.fingerprint == fingerprint
            })
            .unwrap_or(false);
        if unchanged {
            continue;
        }
        handles.remove(&desired.id);
        let reporter = Reporter::new(&desired);
        let cancel = CancellationToken::new();
        let lock = locks
            .entry(desired.id.clone())
            .or_insert_with(|| Arc::new(TokioMutex::new(())))
            .clone();
        let task = spawn_driver(
            desired.clone(),
            reporter.clone(),
            cancel.clone(),
            work_dir.to_path_buf(),
            ctx.clone(),
            lock,
        );
        handles.insert(
            desired.id.clone(),
            Handle {
                revision: desired.revision,
                activation_revision: desired.activation_revision,
                enabled: desired.enabled,
                fingerprint,
                reporter,
                cancel,
                _task: task,
            },
        );
    }
}

fn spawn_driver(
    desired: ExtensionDesired,
    reporter: Reporter,
    cancel: CancellationToken,
    work_dir: PathBuf,
    ctx: HookContext,
    lock: Arc<TokioMutex<()>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        // One lifecycle at a time per extension: if a config change replaces
        // this task, the old driver/deactivate must finish before activate.
        let _serial = lock.lock().await;
        if !desired.enabled {
            reporter.set(ExtensionState::Stopped, "desabilitada pelo control plane");
            cancel.cancelled().await;
            return;
        }

        if let Err(err) = hooks::prepare(&desired, &ctx, &reporter, &cancel).await {
            reporter.set(ExtensionState::Error, err);
            cancel.cancelled().await;
            return;
        }

        match desired.driver.as_str() {
            wm_ext::CLOUDFLARED_DRIVER => {
                match serde_json::from_value::<wm_ext::CloudflaredConfig>(desired.config.clone()) {
                    Ok(config) => {
                        cloudflared::run(
                            config,
                            reporter.clone(),
                            cancel.clone(),
                            work_dir.join("cloudflared"),
                        )
                        .await;
                    }
                    Err(err) => {
                        reporter.set(
                            ExtensionState::Error,
                            format!("config do cloudflared inválida: {err}"),
                        );
                        cancel.cancelled().await;
                    }
                }
            }
            wm_ext::EXEC_DRIVER => {
                match serde_json::from_value::<wm_ext::ExecConfig>(desired.config.clone()) {
                    Ok(config) => {
                        if let Err(err) = config.validate() {
                            reporter.set(ExtensionState::Error, err);
                            cancel.cancelled().await;
                        } else {
                            exec::run(config, reporter.clone(), cancel.clone()).await;
                        }
                    }
                    Err(err) => {
                        reporter.set(ExtensionState::Error, format!("config exec inválida: {err}"));
                        cancel.cancelled().await;
                    }
                }
            }
            other => {
                reporter.set(
                    ExtensionState::Error,
                    format!("driver desconhecido '{other}'; atualize o agente"),
                );
                cancel.cancelled().await;
            }
        }

        hooks::deactivate(&desired, &ctx).await;
    })
}

fn publish(status_tx: &watch::Sender<Vec<ExtensionStatus>>, handles: &HashMap<String, Handle>) {
    let mut statuses: Vec<ExtensionStatus> = handles
        .values()
        .map(|handle| handle.reporter.snapshot())
        .collect();
    statuses.sort_by(|a, b| a.id.cmp(&b.id));
    let changed = *status_tx.borrow() != statuses;
    if changed {
        let _ = status_tx.send(statuses);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use uuid::Uuid;

    fn desired_exec(command: &str, args: Vec<&str>) -> ExtensionDesired {
        ExtensionDesired {
            id: "hello".into(),
            name: "Hello".into(),
            driver: "exec".into(),
            version: "0.1.0".into(),
            revision: 1,
            activation_revision: 0,
            enabled: true,
            hooks: wm_proto::ExtensionHooks::default(),
            config: serde_json::json!({
                "command": command,
                "args": args,
                "restart": "never"
            }),
        }
    }

    fn hook_ctx(dir: &std::path::Path) -> hooks::HookContext {
        hooks::HookContext {
            node_id: Uuid::new_v4(),
            node_name: "test".into(),
            overlay_ip: "10.66.0.2".parse().unwrap(),
            control_url: "http://127.0.0.1:8080".into(),
            data_dir: dir.to_path_buf(),
            extensions_dir: dir.join("extensions"),
        }
    }

    async fn wait_for(
        rx: &mut watch::Receiver<Vec<ExtensionStatus>>,
        predicate: impl Fn(&[ExtensionStatus]) -> bool,
    ) -> Vec<ExtensionStatus> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let current = rx.borrow().clone();
            if predicate(&current) {
                return current;
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            assert!(!remaining.is_zero(), "timeout esperando status; último: {current:?}");
            tokio::time::timeout(remaining, rx.changed())
                .await
                .expect("timeout")
                .expect("supervisor encerrou");
        }
    }

    #[tokio::test]
    async fn disabled_extension_reports_stopped_and_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let mut desired = desired_exec("/bin/true", vec![]);
        desired.enabled = false;
        let (desired_tx, desired_rx) = watch::channel(vec![desired]);
        let (status_tx, mut status_rx) = watch::channel(Vec::new());
        let ctx = hook_ctx(dir.path());
        let handle = tokio::spawn(run(
            desired_rx,
            status_tx,
            dir.path().to_path_buf(),
            ctx,
        ));

        let statuses = wait_for(&mut status_rx, |s| {
            s.len() == 1 && s[0].state == ExtensionState::Stopped
        })
        .await;
        assert_eq!(statuses[0].state, ExtensionState::Stopped);

        desired_tx.send(Vec::new()).unwrap();
        wait_for(&mut status_rx, |s| s.is_empty()).await;
        handle.abort();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exec_driver_goes_running_and_stops_with_the_list() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("sidecar.sh");
        std::fs::write(&script, "#!/bin/sh\necho starting\nsleep 30\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let desired = desired_exec(script.to_str().unwrap(), vec![]);
        let (desired_tx, desired_rx) = watch::channel(vec![desired]);
        let (status_tx, mut status_rx) = watch::channel(Vec::new());
        let ctx = hook_ctx(dir.path());
        let handle = tokio::spawn(run(
            desired_rx,
            status_tx,
            dir.path().to_path_buf(),
            ctx,
        ));

        let statuses = wait_for(&mut status_rx, |s| {
            s.first().map(|status| status.state == ExtensionState::Running).unwrap_or(false)
        })
        .await;
        assert_eq!(statuses[0].driver, "exec");
        let pid = statuses[0].details["pid"].as_u64();
        assert!(pid.is_some_and(|pid| pid > 0));

        desired_tx.send(Vec::new()).unwrap();
        wait_for(&mut status_rx, |s| s.is_empty()).await;
        handle.abort();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn lifecycle_hooks_run_install_activate_and_deactivate() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let scripts = dir.path().join("scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        let write_script = |name: &str, marker: &str| {
            let path = scripts.join(name);
            std::fs::write(
                &path,
                format!(
                    "#!/bin/sh\nprintf '%s' \"$WM_HOOK:$WM_EXTENSION_ID:$WM_NODE_NAME\" > \"$WM_EXTENSION_DIR/{marker}\"\n"
                ),
            )
            .unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        let install = write_script("install.sh", "install.ok");
        let activate = write_script("activate.sh", "activate.ok");
        let deactivate = write_script("deactivate.sh", "deactivate.ok");
        let hook = |path: &std::path::Path| wm_proto::HookSpec {
            command: path.display().to_string(),
            args: Vec::new(),
            env: Default::default(),
            timeout_secs: 5,
            working_dir: None,
        };
        let mut desired = desired_exec("/bin/sleep", vec!["30"]);
        desired.version = "1.2.3".into();
        desired.hooks.install = Some(hook(&install));
        desired.hooks.activate = Some(hook(&activate));
        desired.hooks.deactivate = Some(hook(&deactivate));
        let (desired_tx, desired_rx) = watch::channel(vec![desired]);
        let (status_tx, mut status_rx) = watch::channel(Vec::new());
        let ctx = hook_ctx(dir.path());
        let ext_dir = ctx.extensions_dir.join("hello");
        let handle = tokio::spawn(run(
            desired_rx,
            status_tx,
            dir.path().to_path_buf(),
            ctx,
        ));

        wait_for(&mut status_rx, |s| {
            s.first().map(|status| status.state == ExtensionState::Running).unwrap_or(false)
        })
        .await;
        assert!(ext_dir.join("install.ok").is_file());
        assert!(ext_dir.join("activate.ok").is_file());
        let body = std::fs::read_to_string(ext_dir.join("install.ok")).unwrap();
        assert_eq!(body, "install:hello:test");

        desired_tx.send(Vec::new()).unwrap();
        wait_for(&mut status_rx, |s| s.is_empty()).await;
        for _ in 0..50 {
            if ext_dir.join("deactivate.ok").is_file() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(ext_dir.join("deactivate.ok").is_file(), "deactivate hook não rodou");
        handle.abort();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cloudflared_quick_url_is_parsed_and_reported() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-cloudflared.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\necho 'INF | https://brave-cat-likes-music.trycloudflare.com |'\nsleep 30\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let config = serde_json::json!({
            "binary": script.to_str().unwrap(),
            "quick": [{
                "id": Uuid::new_v4().to_string(),
                "project": "site",
                "local_url": "http://127.0.0.1:3000"
            }]
        });
        let desired = ExtensionDesired {
            id: "cloudflared".into(),
            name: "Cloudflare Tunnel".into(),
            driver: "cloudflared".into(),
            version: "0.1.0".into(),
            revision: 1,
            activation_revision: 0,
            enabled: true,
            hooks: wm_proto::ExtensionHooks::default(),
            config,
        };
        let (desired_tx, desired_rx) = watch::channel(vec![desired]);
        let (status_tx, mut status_rx) = watch::channel(Vec::new());
        let ctx = hook_ctx(dir.path());
        let handle = tokio::spawn(run(
            desired_rx,
            status_tx,
            dir.path().to_path_buf(),
            ctx,
        ));

        let statuses = wait_for(&mut status_rx, |s| {
            s.first()
                .map(|status| {
                    status.state == ExtensionState::Running
                        && !status.public_urls.is_empty()
                })
                .unwrap_or(false)
        })
        .await;
        assert_eq!(
            statuses[0].public_urls,
            vec!["https://brave-cat-likes-music.trycloudflare.com"]
        );
        let details = statuses[0].details["routes"].as_array().unwrap();
        assert_eq!(details[0]["state"], "running");

        desired_tx.send(Vec::new()).unwrap();
        wait_for(&mut status_rx, |s| s.is_empty()).await;
        handle.abort();
    }
}
