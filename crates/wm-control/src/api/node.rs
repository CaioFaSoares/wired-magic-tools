use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use serde::Deserialize;

use wm_proto::{
    ControlEvent, EnrollRequest, EnrollResponse, EndpointsRequest, NetworkMap, NodeStatus,
    PeerInfo, ReportRequest, SelfInfo,
};

use super::{ApiErr, NodeAuth, client_ip};
use crate::auth;
use crate::db;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct MapQuery {
    pub version: Option<u64>,
    pub wait: Option<u64>,
}

pub async fn enroll(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<EnrollRequest>,
) -> Result<Json<EnrollResponse>, ApiErr> {
    let ip = client_ip(&headers).unwrap_or_else(|| "unknown".into());
    if !state.enroll_limiter.allow(&ip) {
        return Err(ApiErr::too_many("muitas tentativas de enrollment"));
    }
    let name = req.name.trim().to_string();
    if !valid_name(&name) {
        return Err(ApiErr::bad_request(
            "invalid_name",
            "nome deve ter 1-63 caracteres: letras, números, '.', '-' ou '_'",
        ));
    }
    let wg_key = decode_wg_key(&req.wg_public_key)
        .ok_or_else(|| ApiErr::bad_request("invalid_key", "chave WireGuard inválida (32 bytes base64)"))?;

    let token_hash = auth::hash_token(&req.join_token);
    if db::name_exists(&state.db, &name).await? {
        return Err(ApiErr::conflict(
            "name_taken",
            "já existe um nó com esse nome; escolha outro",
        ));
    }

    if req.labels.len() > 64 {
        return Err(ApiErr::bad_request("too_many_labels", "no máximo 64 labels"));
    }

    let status = if state.cfg.approval_required {
        NodeStatus::Pending
    } else {
        NodeStatus::Active
    };
    let node_token = auth::random_token("wmn_")?;
    let node_token_hash = auth::hash_token(&node_token);
    let mut req = req;
    req.wg_public_key = wg_key;
    let node = db::enroll_node(
        &state.db, &req, &token_hash, &node_token_hash, status, state.overlay,
    )
    .await
    .map_err(|e| {
        if e.downcast_ref::<db::InvalidJoinToken>().is_some() {
            ApiErr::unauthorized("join_token_invalid", e.to_string())
        } else {
            ApiErr::conflict("enroll_failed", e.to_string())
        }
    })?;

    db::audit(
        &state.db,
        &format!("enroll:{ip}"),
        "node.enroll",
        &node.name,
        Some(&format!("status={:?}", node.status())),
        Some(&ip),
    )
    .await
    .ok();
    super::admin::recompile_acl(&state).await?;
    state.bump_version();
    let view = node.view(db::now_ts());
    state.broadcast(ControlEvent::NodeUpdated(Box::new(view)));

    tracing::info!(node = %node.name, ip = %node.overlay_ip, pending = (status == NodeStatus::Pending), "nó registrado");

    Ok(Json(EnrollResponse {
        node_id: node.uuid(),
        node_token,
        overlay_ip: node.overlay_ip(),
        overlay_cidr: state.network.overlay_cidr,
        node_name: node.name.clone(),
        network_name: state.network.name.clone(),
        control_pubkey: state.control_pubkey.clone(),
    }))
}

pub async fn map(
    State(state): State<Arc<AppState>>,
    auth: NodeAuth,
    Query(q): Query<MapQuery>,
) -> Result<Json<NetworkMap>, ApiErr> {
    if auth.node.status() == NodeStatus::Pending {
        return Err(ApiErr {
            status: axum::http::StatusCode::FORBIDDEN,
            code: "pending_approval".into(),
            message: "nó aguardando aprovação do operador".into(),
            details: Some(serde_json::json!({ "node_id": auth.node.id, "name": auth.node.name })),
        });
    }
    if auth.node.status() != NodeStatus::Active {
        return Err(ApiErr::forbidden("node_inactive", "nó inativo"));
    }

    state.mark_presence(auth.node.uuid(), true).await;
    let mut rx = state.subscribe_version();
    let current = *rx.borrow();
    if q.version == Some(current) {
        let wait = q.wait.unwrap_or(25).clamp(1, 25);
        let _ = tokio::time::timeout(Duration::from_secs(wait), rx.changed()).await;
    }

    let map = build_map(&state, &auth.node).await?;
    Ok(Json(map))
}

pub async fn endpoints(
    State(state): State<Arc<AppState>>,
    auth: NodeAuth,
    Json(req): Json<EndpointsRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    require_active(&auth.node)?;
    if req.candidates.len() > 16 {
        return Err(ApiErr::bad_request(
            "too_many_candidates",
            "no máximo 16 candidatos",
        ));
    }
    let changed = db::update_node_endpoints(
        &state.db,
        &auth.node.id,
        &req.candidates,
        req.nat_type,
        req.relay_connected,
    )
    .await?;
    if changed {
        state.bump_version();
        if let Some(row) = db::node_by_id(&state.db, auth.node.uuid()).await? {
            state.broadcast(ControlEvent::NodeUpdated(Box::new(row.view(db::now_ts()))));
        }
    }
    Ok(Json(serde_json::json!({ "ok": true, "version": state.current_version() })))
}

pub async fn report(
    State(state): State<Arc<AppState>>,
    auth: NodeAuth,
    Json(req): Json<ReportRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    require_active(&auth.node)?;
    db::update_node_report(
        &state.db,
        &auth.node.id,
        &req.peers,
        req.nat_type,
        req.relay_connected,
    )
    .await?;
    if req.extensions.len() > 64 {
        return Err(ApiErr::bad_request(
            "too_many_extensions",
            "no máximo 64 extensões por nó",
        ));
    }
    crate::extensions::store::upsert_statuses(&state.db, auth.node.uuid(), &req.extensions)
        .await?;
    for status in &req.extensions {
        state.broadcast(ControlEvent::ExtensionStatus {
            node_id: auth.node.uuid(),
            extension_id: status.id.clone(),
            state: status.state,
        });
    }
    if let Some(row) = db::node_by_id(&state.db, auth.node.uuid()).await? {
        // Reports refresh presence but do not bump the map version; otherwise
        // every node would wake every long-poll every 30 seconds.
        state.broadcast(ControlEvent::NodeUpdated(Box::new(row.view(db::now_ts()))));
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn leave(
    State(state): State<Arc<AppState>>,
    auth: NodeAuth,
) -> Result<Json<serde_json::Value>, ApiErr> {
    db::set_node_status(&state.db, auth.node.uuid(), NodeStatus::Disabled).await?;
    state.relay.disconnect(auth.node.uuid());
    db::audit(
        &state.db,
        &format!("node:{}", auth.node.name),
        "node.leave",
        &auth.node.name,
        None,
        None,
    )
    .await
    .ok();
    super::admin::recompile_acl(&state).await?;
    state.bump_version();
    state.broadcast(ControlEvent::NodeRemoved {
        id: auth.node.uuid(),
    });
    Ok(Json(serde_json::json!({ "ok": true })))
}

fn require_active(node: &db::NodeRow) -> Result<(), ApiErr> {
    if node.status() == NodeStatus::Active {
        Ok(())
    } else {
        Err(ApiErr::forbidden("node_inactive", "nó não está ativo"))
    }
}

pub async fn build_map(state: &Arc<AppState>, me: &db::NodeRow) -> anyhow::Result<NetworkMap> {
    let now = db::now_ts();
    let nodes = db::list_nodes(&state.db).await?;
    let acl = db::acl_view(&state.db).await?.compiled;
    let peers: Vec<PeerInfo> = nodes
        .iter()
        .filter(|n| n.uuid() != me.uuid() && n.status() == NodeStatus::Active)
        .map(|n| {
            let mut info = n.peer_info(now);
            info.online = info.online || state.relay.is_online(n.uuid());
            info
        })
        .collect();
    let self_info = SelfInfo {
        id: me.uuid(),
        name: me.name.clone(),
        hostname: me.hostname.clone(),
        overlay_ip: me.overlay_ip(),
        wg_public_key: me.wg_public_key.clone(),
        tags: me.tags(),
        labels: me.labels(),
        dns_name: format!("{}.{}", me.name, state.network.dns_zone),
    };
    let extensions = state.extensions.desired_for(state, me).await;
    let shares = super::shares::map_shares(state, &nodes).await;
    Ok(NetworkMap {
        version: state.current_version(),
        protocol: wm_proto::PROTOCOL_VERSION,
        network_name: state.network.name.clone(),
        overlay_cidr: state.network.overlay_cidr,
        dns_zone: state.network.dns_zone.clone(),
        mtu: state.network.mtu,
        me: self_info,
        peers,
        relay: state.relay_info_for(me.uuid()),
        stun: state.cfg.stun_urls.clone(),
        acl,
        extensions,
        shares,
        server_time: chrono::Utc::now(),
    })
}

pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        && !name.starts_with('.')
}

fn decode_wg_key(raw: &str) -> Option<String> {
    let bytes = STANDARD
        .decode(raw.trim())
        .or_else(|_| URL_SAFE_NO_PAD.decode(raw.trim()))
        .ok()?;
    if bytes.len() != 32 {
        return None;
    }
    Some(STANDARD.encode(bytes))
}
