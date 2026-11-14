//! Named services ("shares") API.
//!
//! Two authenticated surfaces:
//!
//! * `/admin/shares` — the operator sees and manages every share;
//! * `/node/shares` — a node manages only its own shares with its node token.
//!
//! The map carries a [`wm_proto::ShareInfo`] view (name, owner, overlay IP,
//! port, kind) so every agent can resolve `wm://name` without another call.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use uuid::Uuid;

use wm_proto::{
    AdminCreateShareRequest, ControlEvent, CreateShareRequest, NodeStatus, NodeView,
    RenameSelfRequest, ShareInfo, ShareKind, ShareProtocol, UpdateProfileRequest,
    UpdateShareRequest, validate_profile, validate_share_description, validate_share_name,
};

use super::{AdminAuth, ApiErr, NodeAuth, client_ip};
use crate::db::{self, NodeRow, ShareRow};
use crate::state::AppState;

struct Normalized {
    name: String,
    port: u16,
    kind: ShareKind,
    protocol: ShareProtocol,
    description: String,
    enabled: bool,
}

fn normalize_new(req: CreateShareRequest) -> Result<Normalized, ApiErr> {
    let req = req.normalized();
    normalized(req.name, req.port, req.kind, req.protocol, req.description, req.enabled)
}

fn normalize_update(current: &ShareRow, req: UpdateShareRequest) -> Result<Normalized, ApiErr> {
    let kind = req.kind.unwrap_or_else(|| current.kind());
    let protocol = req.protocol.unwrap_or_else(|| {
        if req.kind.is_some() {
            default_protocol(kind)
        } else {
            current.protocol()
        }
    });
    normalized(
        req.name.unwrap_or_else(|| current.name.clone()),
        req.port.unwrap_or_else(|| current.port()),
        kind,
        protocol,
        req.description.unwrap_or_else(|| current.description.clone()),
        req.enabled.unwrap_or_else(|| current.enabled()),
    )
}

fn default_protocol(kind: ShareKind) -> ShareProtocol {
    match kind {
        ShareKind::Web => ShareProtocol::Tcp,
        ShareKind::Udp => ShareProtocol::Udp,
        ShareKind::Game => ShareProtocol::Both,
        ShareKind::Tcp => ShareProtocol::Tcp,
    }
}

fn normalized(
    raw_name: String,
    port: u16,
    kind: ShareKind,
    protocol: ShareProtocol,
    description: String,
    enabled: bool,
) -> Result<Normalized, ApiErr> {
    let name = validate_share_name(&raw_name)
        .map_err(|err| ApiErr::bad_request("invalid_share_name", err))?;
    if port == 0 {
        return Err(ApiErr::bad_request(
            "invalid_port",
            "a porta deve estar entre 1 e 65535",
        ));
    }
    let description = validate_share_description(&description)
        .map_err(|err| ApiErr::bad_request("invalid_share_description", err))?;
    // A web share is TCP by definition; a UDP share cannot advertise TCP.
    let protocol = match kind {
        ShareKind::Web => ShareProtocol::Tcp,
        ShareKind::Udp => ShareProtocol::Udp,
        ShareKind::Game => {
            if protocol == ShareProtocol::Tcp {
                ShareProtocol::Both
            } else {
                protocol
            }
        }
        ShareKind::Tcp => protocol,
    };
    Ok(Normalized {
        name,
        port,
        kind,
        protocol,
        description,
        enabled,
    })
}

fn active_node(node: &NodeRow) -> Result<(), ApiErr> {
    match node.status() {
        NodeStatus::Active => Ok(()),
        NodeStatus::Pending => Err(ApiErr::forbidden(
            "node_pending",
            "o nó precisa ser aprovado antes de publicar shares",
        )),
        NodeStatus::Disabled => Err(ApiErr::forbidden(
            "node_disabled",
            "o nó está desativado e não pode publicar shares",
        )),
    }
}

async fn info_for(state: &AppState, row: &ShareRow, node: &NodeRow) -> ShareInfo {
    let mut info = row.info(node, db::now_ts());
    info.online = info.online || state.relay.is_online(node.uuid());
    info
}

async fn infos_for_rows(state: &AppState, rows: Vec<ShareRow>) -> Result<Vec<ShareInfo>, ApiErr> {
    let nodes: HashMap<String, NodeRow> = db::list_nodes(&state.db)
        .await?
        .into_iter()
        .map(|node| (node.id.clone(), node))
        .collect();
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(node) = nodes.get(&row.node_id) else {
            continue;
        };
        out.push(info_for(state, &row, node).await);
    }
    Ok(out)
}

async fn node_info(state: &AppState, row: &ShareRow) -> Result<ShareInfo, ApiErr> {
    let node = db::node_by_id(&state.db, row.node_uuid())
        .await?
        .ok_or_else(|| ApiErr::not_found("node_not_found", "nó dono do share não existe"))?;
    Ok(info_for(state, row, &node).await)
}

async fn broadcast_updated(state: &AppState, row: &ShareRow) -> Result<(), ApiErr> {
    let info = node_info(state, row).await?;
    state.broadcast(ControlEvent::ShareUpdated(Box::new(info)));
    Ok(())
}

async fn require_owner(node: &db::NodeRow, row: &ShareRow) -> Result<(), ApiErr> {
    if row.node_id != node.id {
        return Err(ApiErr::forbidden(
            "not_share_owner",
            "este share pertence a outro nó",
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Admin
// ---------------------------------------------------------------------------

pub async fn list_admin(
    State(state): State<Arc<AppState>>,
    _auth: AdminAuth,
) -> Result<Json<Vec<ShareInfo>>, ApiErr> {
    let rows = db::list_shares(&state.db).await?;
    Ok(Json(infos_for_rows(&state, rows).await?))
}

pub async fn create_admin(
    State(state): State<Arc<AppState>>,
    auth: AdminAuth,
    headers: HeaderMap,
    Json(input): Json<AdminCreateShareRequest>,
) -> Result<Json<ShareInfo>, ApiErr> {
    auth.require_csrf(&headers)?;
    let Some(node) = db::node_by_id(&state.db, input.node_id).await? else {
        return Err(ApiErr::not_found("node_not_found", "nó não encontrado"));
    };
    active_node(&node)?;
    let value = normalize_new(input.share)?;
    if db::share_name_taken(&state.db, &value.name, None).await? {
        return Err(ApiErr::conflict(
            "share_name_taken",
            format!("já existe um share '{}'", value.name),
        ));
    }
    let row = db::create_share(
        &state.db,
        &value.name,
        node.uuid(),
        value.port,
        value.kind,
        value.protocol,
        &value.description,
        value.enabled,
    )
    .await?;
    audit(
        &state,
        &headers,
        &format!("admin:{}", auth.username()),
        "share.create",
        &row,
    )
    .await;
    state.bump_version();
    broadcast_updated(&state, &row).await?;
    Ok(Json(info_for(&state, &row, &node).await))
}

pub async fn update_admin(
    State(state): State<Arc<AppState>>,
    auth: AdminAuth,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateShareRequest>,
) -> Result<Json<ShareInfo>, ApiErr> {
    auth.require_csrf(&headers)?;
    let Some(current) = db::share_by_id(&state.db, id).await? else {
        return Err(ApiErr::not_found("share_not_found", "share não encontrado"));
    };
    let value = normalize_update(&current, req)?;
    if value.name != current.name
        && db::share_name_taken(&state.db, &value.name, Some(id)).await?
    {
        return Err(ApiErr::conflict(
            "share_name_taken",
            format!("já existe um share '{}'", value.name),
        ));
    }
    let row = db::update_share(
        &state.db,
        id,
        &value.name,
        value.port,
        value.kind,
        value.protocol,
        &value.description,
        value.enabled,
    )
    .await?
    .ok_or_else(|| ApiErr::not_found("share_not_found", "share não encontrado"))?;
    audit(
        &state,
        &headers,
        &format!("admin:{}", auth.username()),
        "share.update",
        &row,
    )
    .await;
    state.bump_version();
    broadcast_updated(&state, &row).await?;
    Ok(Json(node_info(&state, &row).await?))
}

pub async fn delete_admin(
    State(state): State<Arc<AppState>>,
    auth: AdminAuth,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    auth.require_csrf(&headers)?;
    let Some(row) = db::delete_share(&state.db, id).await? else {
        return Err(ApiErr::not_found("share_not_found", "share não encontrado"));
    };
    audit(
        &state,
        &headers,
        &format!("admin:{}", auth.username()),
        "share.delete",
        &row,
    )
    .await;
    state.bump_version();
    state.broadcast(ControlEvent::ShareRemoved { id });
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Node self-service
// ---------------------------------------------------------------------------

pub async fn list_own(
    State(state): State<Arc<AppState>>,
    auth: NodeAuth,
) -> Result<Json<Vec<ShareInfo>>, ApiErr> {
    let rows = db::list_shares_for_node(&state.db, auth.node.uuid()).await?;
    Ok(Json(infos_for_rows(&state, rows).await?))
}

pub async fn create_own(
    State(state): State<Arc<AppState>>,
    auth: NodeAuth,
    headers: HeaderMap,
    Json(req): Json<CreateShareRequest>,
) -> Result<Json<ShareInfo>, ApiErr> {
    active_node(&auth.node)?;
    let value = normalize_new(req)?;
    if db::share_name_taken(&state.db, &value.name, None).await? {
        return Err(ApiErr::conflict(
            "share_name_taken",
            format!("já existe um share '{}'", value.name),
        ));
    }
    let row = db::create_share(
        &state.db,
        &value.name,
        auth.node.uuid(),
        value.port,
        value.kind,
        value.protocol,
        &value.description,
        value.enabled,
    )
    .await?;
    audit(
        &state,
        &headers,
        &format!("node:{}", auth.node.name),
        "share.create",
        &row,
    )
    .await;
    state.bump_version();
    broadcast_updated(&state, &row).await?;
    Ok(Json(node_info(&state, &row).await?))
}

pub async fn update_own(
    State(state): State<Arc<AppState>>,
    auth: NodeAuth,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateShareRequest>,
) -> Result<Json<ShareInfo>, ApiErr> {
    active_node(&auth.node)?;
    let Some(current) = db::share_by_id(&state.db, id).await? else {
        return Err(ApiErr::not_found("share_not_found", "share não encontrado"));
    };
    require_owner(&auth.node, &current).await?;
    let value = normalize_update(&current, req)?;
    if value.name != current.name
        && db::share_name_taken(&state.db, &value.name, Some(id)).await?
    {
        return Err(ApiErr::conflict(
            "share_name_taken",
            format!("já existe um share '{}'", value.name),
        ));
    }
    let row = db::update_share(
        &state.db,
        id,
        &value.name,
        value.port,
        value.kind,
        value.protocol,
        &value.description,
        value.enabled,
    )
    .await?
    .ok_or_else(|| ApiErr::not_found("share_not_found", "share não encontrado"))?;
    audit(
        &state,
        &headers,
        &format!("node:{}", auth.node.name),
        "share.update",
        &row,
    )
    .await;
    state.bump_version();
    broadcast_updated(&state, &row).await?;
    Ok(Json(node_info(&state, &row).await?))
}

pub async fn delete_own(
    State(state): State<Arc<AppState>>,
    auth: NodeAuth,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    active_node(&auth.node)?;
    let Some(row) = db::share_by_id(&state.db, id).await? else {
        return Err(ApiErr::not_found("share_not_found", "share não encontrado"));
    };
    require_owner(&auth.node, &row).await?;
    db::delete_share(&state.db, id).await?;
    audit(
        &state,
        &headers,
        &format!("node:{}", auth.node.name),
        "share.delete",
        &row,
    )
    .await;
    state.bump_version();
    state.broadcast(ControlEvent::ShareRemoved { id });
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn rename_self(
    State(state): State<Arc<AppState>>,
    auth: NodeAuth,
    headers: HeaderMap,
    Json(req): Json<RenameSelfRequest>,
) -> Result<Json<NodeView>, ApiErr> {
    let name = req.name.trim();
    if !super::node::valid_name(name) {
        return Err(ApiErr::bad_request("invalid_name", "nome inválido"));
    }
    if name != auth.node.name && db::name_exists(&state.db, name).await? {
        return Err(ApiErr::conflict("name_taken", "nome já está em uso"));
    }
    if name != auth.node.name {
        db::rename_node_self(&state.db, auth.node.uuid(), name).await.map_err(|e| {
            if e.downcast_ref::<db::AclNameReserved>().is_some() {
                ApiErr::forbidden("acl_name_reserved", e.to_string())
            } else {
                ApiErr::conflict("rename_failed", e.to_string())
            }
        })?;
        db::audit(
            &state.db,
            &format!("node:{}", auth.node.name),
            "node.rename",
            &auth.node.name,
            Some(&format!("{} -> {name}", auth.node.name)),
            client_ip(&headers).as_deref(),
        )
        .await
        .ok();
        super::admin::recompile_acl(&state).await?;
        state.bump_version();
    }
    let row = db::node_by_id(&state.db, auth.node.uuid())
        .await?
        .ok_or_else(|| ApiErr::not_found("node_not_found", "nó não encontrado"))?;
    let view = row.view(db::now_ts());
    state.broadcast(ControlEvent::NodeUpdated(Box::new(view.clone())));
    Ok(Json(view))
}

/// Presentation identity: never touches the technical name used by ACL.
pub async fn update_profile(
    State(state): State<Arc<AppState>>,
    auth: NodeAuth,
    headers: HeaderMap,
    Json(req): Json<UpdateProfileRequest>,
) -> Result<Json<NodeView>, ApiErr> {
    let profile = validate_profile(&req.display_name, &req.about)
        .map_err(|err| ApiErr::bad_request("invalid_profile", err))?;
    db::update_node_profile(
        &state.db,
        auth.node.uuid(),
        &profile.display_name,
        &profile.about,
    )
    .await?;
    db::audit(
        &state.db,
        &format!("node:{}", auth.node.name),
        "node.profile",
        &auth.node.name,
        Some(&format!("display_name={}", profile.display_name)),
        client_ip(&headers).as_deref(),
    )
    .await
    .ok();
    // Peers see the new identity on their next map: bump the version.
    state.bump_version();
    let row = db::node_by_id(&state.db, auth.node.uuid())
        .await?
        .ok_or_else(|| ApiErr::not_found("node_not_found", "nó não encontrado"))?;
    let view = row.view(db::now_ts());
    state.broadcast(ControlEvent::NodeUpdated(Box::new(view.clone())));
    Ok(Json(view))
}

// ---------------------------------------------------------------------------
// Map projection
// ---------------------------------------------------------------------------

/// Shares visible to every active agent. Disabled shares stay in the list so
/// the owner can toggle them from the TUI/GUI; consumers must check `enabled`.
pub(crate) async fn map_shares(state: &AppState, nodes: &[NodeRow]) -> Vec<ShareInfo> {
    let rows = match db::list_shares(&state.db).await {
        Ok(rows) => rows,
        Err(err) => {
            tracing::warn!(%err, "falha ao listar shares para o mapa");
            return Vec::new();
        }
    };
    let by_id: HashMap<String, &NodeRow> = nodes
        .iter()
        .filter(|node| node.status() == NodeStatus::Active)
        .map(|node| (node.id.clone(), node))
        .collect();
    let now = db::now_ts();
    let mut out = Vec::new();
    for row in rows {
        let Some(node) = by_id.get(&row.node_id) else {
            continue;
        };
        let mut info = row.info(node, now);
        info.online = info.online || state.relay.is_online(node.uuid());
        out.push(info);
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

async fn audit(
    state: &AppState,
    headers: &HeaderMap,
    actor: &str,
    action: &str,
    row: &ShareRow,
) {
    let detail = format!(
        "{}:{} kind={} proto={} enabled={}",
        row.name,
        row.port,
        row.kind,
        row.protocol,
        row.enabled()
    );
    db::audit(
        &state.db,
        actor,
        action,
        &row.name,
        Some(&detail),
        client_ip(headers).as_deref(),
    )
    .await
    .ok();
}

/// Reserved for the cloudflared integration: surfaces a public URL for a share.
#[allow(dead_code)]
pub async fn set_public_url(
    state: &AppState,
    id: Uuid,
    public_url: Option<&str>,
) -> anyhow::Result<()> {
    db::set_share_public_url(&state.db, id, public_url).await
}

#[allow(dead_code)]
fn status_of(row: &ShareRow) -> StatusCode {
    if row.enabled() {
        StatusCode::OK
    } else {
        StatusCode::NO_CONTENT
    }
}
