//! Local agent API over a Unix domain socket. Newline-delimited JSON in, one
//! JSON response line out. Only local processes with access to the socket can
//! use it; the daemon runs as root, so the socket is 0660.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpStream, UnixListener, UnixStream};
use tokio::sync::{RwLock, mpsc, oneshot, watch};
use uuid::Uuid;

use wm_proto::{
    CreateShareRequest, ExtensionStatus, LocalRequest, LocalResponse, PeerStatus, ShareInfo,
    UpdateShareRequest,
};

use crate::agent::{AgentSnapshot, Cmd};
use crate::control::ControlClient;
use crate::state::NodeState;

pub(crate) struct LocalControl {
    pub client: ControlClient,
    pub token: String,
    pub state_path: PathBuf,
}

pub(crate) async fn serve(
    path: PathBuf,
    snapshot: Arc<RwLock<AgentSnapshot>>,
    cmd_tx: mpsc::Sender<Cmd>,
    extension_status: watch::Receiver<Vec<ExtensionStatus>>,
    extension_logs: crate::extensions::logs::ExtensionLogs,
    control: ControlClient,
    token: String,
    state_path: PathBuf,
) -> anyhow::Result<()> {
    if path.exists() {
        if tokio::net::UnixStream::connect(&path).await.is_ok() {
            anyhow::bail!(
                "já existe um agente rodando em {}; pare-o antes",
                path.display()
            );
        }
        let _ = std::fs::remove_file(&path);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = UnixListener::bind(&path)
        .with_context(|| format!("bind do socket {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o660));
    }
    tracing::info!(path = %path.display(), "API local ouvindo");
    let control = Arc::new(LocalControl {
        client: control,
        token,
        state_path,
    });
    loop {
        let Ok((stream, _addr)) = listener.accept().await else {
            continue;
        };
        let snapshot = snapshot.clone();
        let cmd_tx = cmd_tx.clone();
        let extension_status = extension_status.clone();
        let control = control.clone();
        let extension_logs = extension_logs.clone();
        tokio::spawn(async move {
            if let Err(err) = handle(stream, snapshot, cmd_tx, extension_status, extension_logs, control).await {
                tracing::debug!(%err, "conexão da API local encerrada");
            }
        });
    }
}

async fn handle(
    stream: UnixStream,
    snapshot: Arc<RwLock<AgentSnapshot>>,
    cmd_tx: mpsc::Sender<Cmd>,
    extension_status: watch::Receiver<Vec<ExtensionStatus>>,
    extension_logs: crate::extensions::logs::ExtensionLogs,
    control: Arc<LocalControl>,
) -> anyhow::Result<()> {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<LocalRequest>(&line) {
            Ok(req) => {
                let id = req.id;
                match dispatch(
                    req.method.as_str(),
                    req.params,
                    &snapshot,
                    &cmd_tx,
                    &extension_status,
                    &extension_logs,
                    &control,
                )
                .await
                {
                    Ok(value) => LocalResponse::ok(id, value),
                    Err(err) => LocalResponse::err(id, err),
                }
            }
            Err(err) => LocalResponse::err(0, format!("requisição inválida: {err}")),
        };
        let mut out = serde_json::to_vec(&response)?;
        out.push(b'\n');
        write.write_all(&out).await?;
        write.flush().await?;
    }
    Ok(())
}

async fn dispatch(
    method: &str,
    params: Value,
    snapshot: &Arc<RwLock<AgentSnapshot>>,
    cmd_tx: &mpsc::Sender<Cmd>,
    extension_status: &watch::Receiver<Vec<ExtensionStatus>>,
    extension_logs: &crate::extensions::logs::ExtensionLogs,
    control: &LocalControl,
) -> Result<Value, String> {
    match method {
        "shutdown" => {
            let tx = cmd_tx.clone();
            tokio::spawn(async move {
                // Allow the acknowledgment to reach the caller before the API
                // listener task is aborted by the normal shutdown path.
                tokio::time::sleep(Duration::from_millis(100)).await;
                let _ = tx.send(Cmd::Shutdown).await;
            });
            Ok(json!({"stopping": true}))
        }
        wm_ext::CONTEXT_METHOD => {
            let snap = snapshot.read().await;
            let context = wm_ext::ExtensionContext {
                api_version: wm_ext::CONTEXT_API_VERSION,
                host: snap.status.clone(),
                nodes: snap.peers.clone(),
                shares: snap.shares.clone(),
                extensions: extension_status.borrow().clone(),
            };
            serde_json::to_value(context).map_err(|e| e.to_string())
        }
        "status" => {
            let snap = snapshot.read().await;
            serde_json::to_value(&snap.status).map_err(|e| e.to_string())
        }
        "peers" => {
            let snap = snapshot.read().await;
            serde_json::to_value(&snap.peers).map_err(|e| e.to_string())
        }
        "shares" => {
            let snap = snapshot.read().await;
            serde_json::to_value(&snap.shares).map_err(|e| e.to_string())
        }
        "share_resolve" => {
            let raw = params
                .get("name")
                .and_then(Value::as_str)
                .ok_or("informe name")?;
            let name = raw
                .trim()
                .strip_prefix("wm://")
                .unwrap_or(raw.trim())
                .trim_end_matches('/');
            let snap = snapshot.read().await;
            let share = snap
                .shares
                .iter()
                .find(|share| share.name == name || share.id.to_string() == name)
                .ok_or_else(|| format!("share '{name}' não encontrado"))?;
            if !share.enabled {
                return Err(format!("share '{name}' está desligado"));
            }
            Ok(json!({
                "share": share,
                "address": share.address(),
                "url": share.open_url(),
                "wm_url": share.wm_url(),
            }))
        }
        "share_create" => {
            let req: CreateShareRequest = serde_json::from_value(params)
                .map_err(|err| format!("share inválido: {err}"))?;
            let share = control
                .client
                .create_share(&control.token, &req)
                .await
                .map_err(|err| err.to_string())?;
            upsert_share(snapshot, share.clone()).await;
            Ok(json!({
                "share": share,
                "address": share.address(),
                "url": share.open_url(),
                "wm_url": share.wm_url(),
            }))
        }
        "share_update" => {
            let id = params
                .get("id")
                .and_then(Value::as_str)
                .map(|raw| Uuid::parse_str(raw).map_err(|_| "id inválido".to_string()))
                .transpose()?;
            let id = match id {
                Some(id) => id,
                None => {
                    let name = params
                        .get("name")
                        .and_then(Value::as_str)
                        .ok_or("informe id ou name")?;
                    own_share_id(snapshot, name).await?
                }
            };
            let req: UpdateShareRequest = serde_json::from_value(params)
                .map_err(|err| format!("atualização inválida: {err}"))?;
            let share = control
                .client
                .update_share(&control.token, id, &req)
                .await
                .map_err(|err| err.to_string())?;
            upsert_share(snapshot, share.clone()).await;
            Ok(json!({ "share": share }))
        }
        "share_delete" => {
            let id = {
                let raw = params.get("id").and_then(Value::as_str);
                match raw {
                    Some(raw) => Uuid::parse_str(raw).map_err(|_| "id inválido".to_string())?,
                    None => {
                        let name = params
                            .get("name")
                            .and_then(Value::as_str)
                            .ok_or("informe id ou name")?;
                        own_share_id(snapshot, name).await?
                    }
                }
            };
            control
                .client
                .delete_share(&control.token, id)
                .await
                .map_err(|err| err.to_string())?;
            let mut snap = snapshot.write().await;
            snap.shares.retain(|share| share.id != id);
            Ok(json!({ "ok": true }))
        }
        "rename_node" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .ok_or("informe name")?
                .trim()
                .to_string();
            if name.is_empty() {
                return Err("nome vazio".into());
            }
            let view = control
                .client
                .rename_self(&control.token, &name)
                .await
                .map_err(|err| err.to_string())?;
            let new_name = view.node.name.clone();
            if let Ok(Some(mut state)) = NodeState::load(&control.state_path) {
                state.node_name = new_name.clone();
                if let Err(err) = state.save(&control.state_path) {
                    tracing::warn!(%err, "não foi possível persistir o nome do nó");
                }
            }
            let mut snap = snapshot.write().await;
            let self_id = snap.status.node_id;
            snap.status.node_name = Some(new_name.clone());
            snap.self_name = new_name.clone();
            for share in &mut snap.shares {
                if Some(share.node_id) == self_id {
                    share.node_name = new_name.clone();
                }
            }
            Ok(json!({ "ok": true, "name": new_name }))
        }
        "set_profile" => {
            let display_name = params
                .get("display_name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let about = params.get("about").and_then(Value::as_str).unwrap_or_default();
            let profile = wm_proto::validate_profile(display_name, about)?;
            let view = control
                .client
                .update_profile(&control.token, &profile)
                .await
                .map_err(|err| err.to_string())?;
            let new_name = view.node.display_name.clone();
            let new_about = view.node.about.clone();
            if let Ok(Some(mut state)) = NodeState::load(&control.state_path) {
                state.display_name = new_name.clone();
                state.about = new_about.clone();
                if let Err(err) = state.save(&control.state_path) {
                    tracing::warn!(%err, "não foi possível persistir o perfil do nó");
                }
            }
            let mut snap = snapshot.write().await;
            snap.status.display_name = new_name.clone();
            snap.status.about = new_about.clone();
            Ok(json!({ "ok": true, "display_name": new_name, "about": new_about }))
        }
        "resolve" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .ok_or("informe name")?;
            let snap = snapshot.read().await;
            if name == snap.self_name {
                return Ok(json!({ "name": name, "ip": snap.self_ip }));
            }
            if let Some((_, ip)) = snap.peer_ips.get(name) {
                return Ok(json!({ "name": name, "ip": ip }));
            }
            Err(format!("nó '{name}' não encontrado"))
        }
        "probe" => {
            let peer = params
                .get("peer")
                .and_then(Value::as_str)
                .ok_or("informe peer")?;
            let port = params
                .get("port")
                .and_then(Value::as_u64)
                .ok_or("informe port")? as u16;
            if port == 0 {
                return Err("porta inválida".into());
            }
            let (ip, path) = {
                let snap = snapshot.read().await;
                resolve_peer(&snap, peer).ok_or_else(|| format!("nó '{peer}' não encontrado"))?
            };
            let started = std::time::Instant::now();
            let result = tokio::time::timeout(
                Duration::from_secs(5),
                TcpStream::connect((ip, port)),
            )
            .await;
            let elapsed = started.elapsed().as_secs_f64() * 1000.0;
            let out = match result {
                Ok(Ok(_)) => json!({
                    "peer": peer, "overlay_ip": ip, "port": port, "connected": true,
                    "elapsed_ms": elapsed, "path": path, "error": Value::Null
                }),
                Ok(Err(err)) => json!({
                    "peer": peer, "overlay_ip": ip, "port": port, "connected": false,
                    "elapsed_ms": elapsed, "path": path, "error": err.to_string()
                }),
                Err(_) => json!({
                    "peer": peer, "overlay_ip": ip, "port": port, "connected": false,
                    "elapsed_ms": elapsed, "path": path, "error": "timeout"
                }),
            };
            Ok(out)
        }
        "ping" => {
            let peer = params
                .get("peer")
                .and_then(Value::as_str)
                .ok_or("informe peer")?
                .to_string();
            let (tx, rx) = oneshot::channel();
            cmd_tx
                .send(Cmd::Ping { peer, reply: tx })
                .await
                .map_err(|_| "agente parado".to_string())?;
            match tokio::time::timeout(Duration::from_secs(8), rx).await {
                Ok(Ok(Ok(result))) => serde_json::to_value(result).map_err(|e| e.to_string()),
                Ok(Ok(Err(err))) => Err(err),
                Ok(Err(_)) => Err("ping cancelado".into()),
                Err(_) => Err("ping sem resposta (timeout)".into()),
            }
        }
        "reconnect" => {
            let peer = params
                .get("peer")
                .and_then(Value::as_str)
                .ok_or("informe peer")?
                .to_string();
            let (tx, rx) = oneshot::channel();
            cmd_tx
                .send(Cmd::Reconnect { peer, reply: tx })
                .await
                .map_err(|_| "agente parado".to_string())?;
            match tokio::time::timeout(Duration::from_secs(3), rx).await {
                Ok(Ok(Ok(()))) => Ok(json!({ "ok": true })),
                Ok(Ok(Err(err))) => Err(err),
                Ok(Err(_)) => Err("reconexão cancelada".into()),
                Err(_) => Err("timeout ao reconectar".into()),
            }
        }
        "extension_logs" => extension_logs.read(params),
        "extensions" => Ok(serde_json::to_value(&*extension_status.borrow())
            .map_err(|err| err.to_string())?),
        "extension" => {
            let id = params
                .get("id")
                .and_then(Value::as_str)
                .ok_or("informe id")?;
            extension_status
                .borrow()
                .iter()
                .find(|status| status.id == id)
                .map(|status| serde_json::to_value(status).map_err(|err| err.to_string()))
                .unwrap_or_else(|| Err(format!("extensão '{id}' não encontrada no nó")))
        }
        "extension_catalog" => {
            let catalog = control
                .client
                .node_extensions(&control.token)
                .await
                .map_err(|err| err.to_string())?;
            serde_json::to_value(catalog).map_err(|err| err.to_string())
        }
        "extension_activation" => {
            let id = params
                .get("id")
                .and_then(Value::as_str)
                .ok_or("informe id")?;
            let active = params
                .get("active")
                .and_then(Value::as_bool)
                .ok_or("informe active")?;
            let view = control
                .client
                .set_node_extension(&control.token, id, active)
                .await
                .map_err(|err| err.to_string())?;
            serde_json::to_value(view).map_err(|err| err.to_string())
        }
        "logs" => Ok(json!({
            "lines": [],
            "note": "use `journalctl -u wired-magic-agent` (Linux) ou Console.app (macOS)"
        })),
        "metrics" => {
            let snap = snapshot.read().await;
            Ok(json!({
                "peers_total": snap.status.peers_total,
                "peers_online": snap.status.peers_online,
                "map_version": snap.status.map_version,
                "control_connected": snap.status.control_connected,
                "relay_connected": snap.status.relay_connected,
                "uptime_secs": snap.status.uptime_secs,
            }))
        }
        other => Err(format!("método desconhecido: {other}")),
    }
}

async fn own_share_id(snapshot: &Arc<RwLock<AgentSnapshot>>, name: &str) -> Result<Uuid, String> {
    let name = name
        .trim()
        .strip_prefix("wm://")
        .unwrap_or(name.trim())
        .trim_end_matches('/');
    let snap = snapshot.read().await;
    let self_id = snap.status.node_id.ok_or("nó sem identidade")?;
    snap.shares
        .iter()
        .find(|share| share.node_id == self_id && (share.name == name || share.id.to_string() == name))
        .map(|share| share.id)
        .ok_or_else(|| format!("share '{name}' não pertence a este nó"))
}

async fn upsert_share(snapshot: &Arc<RwLock<AgentSnapshot>>, share: ShareInfo) {
    let mut snap = snapshot.write().await;
    if let Some(existing) = snap.shares.iter_mut().find(|item| item.id == share.id) {
        *existing = share;
    } else {
        snap.shares.push(share);
    }
    snap.shares
        .sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
}

fn resolve_peer(snap: &AgentSnapshot, peer: &str) -> Option<(std::net::Ipv4Addr, wm_proto::PathKind)> {
    if let Ok(id) = Uuid::parse_str(peer) {
        if let Some(p) = snap.peers.iter().find(|p| p.id == id) {
            return Some((p.overlay_ip, p.path));
        }
    }
    if let Some(p) = snap.peers.iter().find(|p| p.name == peer) {
        return Some((p.overlay_ip, p.path));
    }
    snap.peer_ips.get(peer).map(|(id, ip)| {
        let path = snap
            .peers
            .iter()
            .find(|p| p.id == *id)
            .map(|p| p.path)
            .unwrap_or_default();
        (*ip, path)
    })
}

#[allow(dead_code)]
fn _peer_status(v: PeerStatus) -> PeerStatus {
    v
}

#[allow(dead_code)]
fn _path(_: &Path) {}
