use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use futures_util::stream;
use serde::Deserialize;
use tokio::sync::broadcast;
use uuid::Uuid;

use wm_proto::{
    AclPolicy, AclView, AdminUser, AuditEventView, ControlEvent, CreateJoinTokenRequest,
    CreateServiceTokenRequest, JoinTokenView, LoginRequest, LoginResponse, NodeStatus, NodeView,
    NodeRequirements, Overview, ServiceTokenView, SettingsView, SimulateRequest,
    SimulateResponse, UpdateNodeRequest,
};

use super::{AdminAuth, ApiErr, client_ip};
use crate::acl;
use crate::auth;
use crate::db;
use crate::state::AppState;

const SERVICE_SCOPE: &str = "integration:nodes:read";

pub async fn login(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> Result<Response, ApiErr> {
    let admin = super::accounts::authenticate(&state, &headers, req, true).await?;
    let (session_token, csrf) = super::accounts::issue_session(&state, &headers, &admin).await?;
    let ip = client_ip(&headers).unwrap_or_else(|| "unknown".into());
    db::audit(
        &state.db,
        &format!("admin:{}", admin.username),
        "admin.login",
        &admin.username,
        None,
        Some(&ip),
    )
    .await
    .ok();

    let body = Json(LoginResponse {
        user: AdminUser {
            id: admin.uuid(),
            username: admin.username.clone(),
        },
        csrf_token: csrf,
    })
    .into_response();
    let cookie = auth::session_cookie(&session_token, auth::SESSION_TTL_SECS, state.is_https());
    let mut response = body;
    response.headers_mut().insert(
        header::SET_COOKIE,
        cookie.parse().expect("cookie header"),
    );
    Ok(response)
}

pub async fn logout(
    State(state): State<Arc<AppState>>,
    auth: AdminAuth,
    headers: HeaderMap,
) -> Result<Response, ApiErr> {
    auth.require_csrf(&headers)?;
    db::delete_session(&state.db, &auth.session.id).await?;
    db::audit(
        &state.db,
        &format!("admin:{}", auth.username()),
        "admin.logout",
        auth.username(),
        None,
        client_ip(&headers).as_deref(),
    )
    .await
    .ok();
    let mut response = Json(serde_json::json!({ "ok": true })).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        auth::clear_session_cookie(state.is_https())
            .parse()
            .expect("cookie"),
    );
    Ok(response)
}

pub async fn me(auth: AdminAuth) -> Json<AdminUser> {
    Json(AdminUser {
        id: auth.admin.uuid(),
        username: auth.admin.username.clone(),
    })
}

/// Returns the current admin and the CSRF token of this session. The panel uses
/// it after a reload to keep the HttpOnly cookie session without re-login.
pub async fn session(auth: AdminAuth) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "user": AdminUser {
            id: auth.admin.uuid(),
            username: auth.admin.username.clone(),
        },
        "csrf_token": auth.session.csrf_token,
    }))
}

pub async fn overview(
    State(state): State<Arc<AppState>>,
    auth: AdminAuth,
) -> Result<Json<Overview>, ApiErr> {
    let now = db::now_ts();
    let nodes = db::list_nodes(&state.db).await?;
    let mut nodes_online = 0;
    let mut nodes_pending = 0;
    let mut direct_pairs = 0usize;
    let mut relay_pairs = 0usize;
    for node in &nodes {
        let online = node.online(now) || state.relay.is_online(node.uuid());
        nodes_online += usize::from(online);
        nodes_pending += usize::from(node.status() == NodeStatus::Pending);
        for peer in node.reported_peers() {
            match peer.path {
                wm_proto::PathKind::Direct => direct_pairs += 1,
                wm_proto::PathKind::Relay => relay_pairs += 1,
                _ => {}
            }
        }
    }
    Ok(Json(Overview {
        network: state.network.clone(),
        version: state.current_version(),
        nodes_total: nodes.len(),
        nodes_online,
        nodes_pending,
        relay_connected: state.relay.online_count(),
        direct_pairs: direct_pairs / 2,
        relay_pairs: relay_pairs / 2,
        control_uptime_secs: state.started.elapsed().as_secs(),
        admin: AdminUser {
            id: auth.admin.uuid(),
            username: auth.admin.username.clone(),
        },
    }))
}

pub async fn list_nodes(
    State(state): State<Arc<AppState>>,
    _auth: AdminAuth,
) -> Result<Json<Vec<NodeView>>, ApiErr> {
    let now = db::now_ts();
    let rows = db::list_nodes(&state.db).await?;
    Ok(Json(rows.iter().map(|r| r.view(now)).collect()))
}

pub async fn approve_node(
    State(state): State<Arc<AppState>>,
    auth: AdminAuth,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<NodeView>, ApiErr> {
    auth.require_csrf(&headers)?;
    let Some(node) = db::node_by_id(&state.db, id).await? else {
        return Err(ApiErr::not_found("node_not_found", "nó não encontrado"));
    };
    db::set_node_status(&state.db, id, NodeStatus::Active).await?;
    if let Err(err) = recompile_acl(&state).await {
        tracing::warn!(%err, "recompilar ACL após aprovação");
    }
    db::audit(
        &state.db,
        &format!("admin:{}", auth.username()),
        "node.approve",
        &node.name,
        None,
        client_ip(&headers).as_deref(),
    )
    .await
    .ok();
    state.bump_version();
    let row = db::node_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiErr::not_found("node_not_found", "nó não encontrado"))?;
    let view = row.view(db::now_ts());
    state.broadcast(ControlEvent::NodeUpdated(Box::new(view.clone())));
    Ok(Json(view))
}

pub async fn update_node(
    State(state): State<Arc<AppState>>,
    auth: AdminAuth,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateNodeRequest>,
) -> Result<Json<NodeView>, ApiErr> {
    auth.require_csrf(&headers)?;
    let Some(current) = db::node_by_id(&state.db, id).await? else {
        return Err(ApiErr::not_found("node_not_found", "nó não encontrado"));
    };
    if let Some(name) = req.name.as_deref() {
        let name = name.trim();
        if !super::node::valid_name(name) {
            return Err(ApiErr::bad_request("invalid_name", "nome inválido"));
        }
        if name != current.name && db::name_exists(&state.db, name).await? {
            return Err(ApiErr::conflict("name_taken", "nome já está em uso"));
        }
    }
    db::update_node(
        &state.db,
        id,
        req.name.as_deref().map(str::trim),
        req.tags.as_deref(),
        req.labels.as_ref(),
        req.status,
    )
    .await?;
    if req.tags.is_some() || req.name.is_some() || req.status.is_some() {
        if let Err(err) = recompile_acl(&state).await {
            tracing::warn!(%err, "recompilar ACL após edição de nó");
        }
    }
    db::audit(
        &state.db,
        &format!("admin:{}", auth.username()),
        "node.update",
        &current.name,
        Some(&serde_json::to_string(&req).unwrap_or_default()),
        client_ip(&headers).as_deref(),
    )
    .await
    .ok();
    state.bump_version();
    let row = db::node_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiErr::not_found("node_not_found", "nó não encontrado"))?;
    let view = row.view(db::now_ts());
    state.broadcast(ControlEvent::NodeUpdated(Box::new(view.clone())));
    Ok(Json(view))
}

/// Replaces the per-node requirements. Empty requirements clear the policy.
/// Compliance is recomputed immediately from the last observed report so the
/// panel does not wait for the next 30 s heartbeat.
pub async fn set_node_requirements(
    State(state): State<Arc<AppState>>,
    auth: AdminAuth,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(mut req): Json<NodeRequirements>,
) -> Result<Json<NodeView>, ApiErr> {
    auth.require_csrf(&headers)?;
    let Some(current) = db::node_by_id(&state.db, id).await? else {
        return Err(ApiErr::not_found("node_not_found", "nó não encontrado"));
    };
    normalize_requirements(&mut req);
    validate_requirements(&req).map_err(|err| ApiErr::bad_request("invalid_requirements", err))?;
    db::set_node_requirements(&state.db, id, &req).await?;
    let statuses: Vec<wm_proto::ExtensionStatus> =
        crate::extensions::store::list_statuses_for_node(&state.db, id)
            .await?
            .iter()
            .map(|row| row.view())
            .collect();
    let compliance = crate::compliance::evaluate(
        current.version.as_deref().unwrap_or_default(),
        &current.tags(),
        &current.agent_runtime(),
        &statuses,
        &req,
    );
    db::set_node_compliance(&state.db, id, &compliance).await?;
    db::audit(
        &state.db,
        &format!("admin:{}", auth.username()),
        "node.requirements",
        &current.name,
        Some(&serde_json::to_string(&req).unwrap_or_default()),
        client_ip(&headers).as_deref(),
    )
    .await
    .ok();
    state.bump_version();
    let row = db::node_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiErr::not_found("node_not_found", "nó não encontrado"))?;
    let view = row.view(db::now_ts());
    state.broadcast(ControlEvent::NodeUpdated(Box::new(view.clone())));
    Ok(Json(view))
}

/// Empty strings/vectors from the panel mean "no requirement", not "require
/// empty". Normalize before validating/storing.
fn normalize_requirements(req: &mut NodeRequirements) {
    if req.min_version.as_deref().is_some_and(|value| value.trim().is_empty()) {
        req.min_version = None;
    }
    if req.agent.tun_name.as_deref().is_some_and(|value| value.trim().is_empty()) {
        req.agent.tun_name = None;
    }
    if req.agent.expose.as_ref().is_some_and(Vec::is_empty) {
        req.agent.expose = None;
    }
    req.required_tags = req
        .required_tags
        .iter()
        .map(|tag| tag.trim().to_string())
        .filter(|tag| !tag.is_empty())
        .collect();
    req.required_extensions = req
        .required_extensions
        .iter()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .collect();
}

fn validate_requirements(req: &NodeRequirements) -> Result<(), String> {
    if let Some(version) = req.min_version.as_deref() {
        let version = version.trim();
        if version.is_empty() || version.chars().count() > 32 || version.chars().any(char::is_control) {
            return Err("versão mínima inválida".into());
        }
    }
    if req.required_extensions.len() > 16 {
        return Err("no máximo 16 extensões obrigatórias".into());
    }
    for ext in &req.required_extensions {
        if ext.is_empty()
            || ext.len() > 64
            || !ext
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(format!("id de extensão inválido: '{ext}'"));
        }
    }
    if req.required_tags.len() > 16 {
        return Err("no máximo 16 tags obrigatórias".into());
    }
    for tag in &req.required_tags {
        let tag = tag.trim();
        if tag.is_empty() || tag.len() > 64 || tag.chars().any(char::is_control) {
            return Err(format!("tag inválida: '{tag}'"));
        }
    }
    if req.agent.udp_port == Some(0) {
        return Err("porta UDP 0 não é válida".into());
    }
    if let Some(mtu) = req.agent.mtu {
        if !(576..=9000).contains(&mtu) {
            return Err("MTU deve estar entre 576 e 9000".into());
        }
    }
    if let Some(expose) = &req.agent.expose {
        if expose.len() > 16 {
            return Err("no máximo 16 portas expostas exigidas".into());
        }
        for spec in expose {
            if crate::compliance::normalize_expose(spec).is_none() {
                return Err(format!("porta exposta inválida: '{spec}'"));
            }
        }
    }
    if let Some(tun) = req.agent.tun_name.as_deref() {
        let tun = tun.trim();
        if tun.is_empty()
            || tun.len() > 15
            || !tun
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err("nome de TUN inválido".into());
        }
    }
    Ok(())
}

pub async fn revoke_node(    State(state): State<Arc<AppState>>,
    auth: AdminAuth,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    auth.require_csrf(&headers)?;
    let Some(node) = db::node_by_id(&state.db, id).await? else {
        return Err(ApiErr::not_found("node_not_found", "nó não encontrado"));
    };
    db::set_node_status(&state.db, id, NodeStatus::Disabled).await?;
    state.relay.disconnect(id);
    if let Err(err) = recompile_acl(&state).await {
        tracing::warn!(%err, "recompilar ACL após revogação");
    }
    db::audit(
        &state.db,
        &format!("admin:{}", auth.username()),
        "node.revoke",
        &node.name,
        None,
        client_ip(&headers).as_deref(),
    )
    .await
    .ok();
    state.bump_version();
    state.broadcast(ControlEvent::NodeRemoved { id });
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn list_join_tokens(
    State(state): State<Arc<AppState>>,
    _auth: AdminAuth,
) -> Result<Json<Vec<JoinTokenView>>, ApiErr> {
    let rows = db::list_join_tokens(&state.db).await?;
    Ok(Json(rows.iter().map(|r| r.view()).collect()))
}

pub async fn create_join_token(
    State(state): State<Arc<AppState>>,
    auth: AdminAuth,
    headers: HeaderMap,
    Json(req): Json<CreateJoinTokenRequest>,
) -> Result<Json<JoinTokenView>, ApiErr> {
    auth.require_csrf(&headers)?;
    let name = req.name.trim();
    if name.is_empty() || name.len() > 64 {
        return Err(ApiErr::bad_request("invalid_name", "nome do token inválido"));
    }
    let ttl = req.ttl_secs.clamp(60, 30 * 24 * 60 * 60);
    let uses = req.max_uses.clamp(1, 1000);
    let token = auth::random_token("wmj_")?;
    let hash = auth::hash_token(&token);
    let prefix: String = token.chars().take(12).collect();
    let row = db::create_join_token_with_approval(
        &state.db,
        name,
        &prefix,
        &hash,
        &req.tags,
        &req.labels,
        uses,
        ttl,
        req.require_approval,
        &format!("admin:{}", auth.username()),
    )
    .await?;
    db::audit(
        &state.db,
        &format!("admin:{}", auth.username()),
        "join_token.create",
        &row.id,
        Some(name),
        client_ip(&headers).as_deref(),
    )
    .await
    .ok();
    let mut view = row.view();
    view.token = Some(token);
    state.broadcast(ControlEvent::JoinTokenCreated(Box::new(view.clone())));
    Ok(Json(view))
}

pub async fn revoke_join_token(
    State(state): State<Arc<AppState>>,
    auth: AdminAuth,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    auth.require_csrf(&headers)?;
    db::revoke_join_token(&state.db, id).await?;
    db::audit(
        &state.db,
        &format!("admin:{}", auth.username()),
        "join_token.revoke",
        &id.to_string(),
        None,
        client_ip(&headers).as_deref(),
    )
    .await
    .ok();
    state.broadcast(ControlEvent::JoinTokenRevoked { id });
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn get_acl(
    State(state): State<Arc<AppState>>,
    _auth: AdminAuth,
) -> Result<Json<AclView>, ApiErr> {
    Ok(Json(db::acl_view(&state.db).await?))
}

#[derive(Debug, Deserialize)]
pub struct AclUpdate {
    pub yaml: String,
}

pub async fn put_acl(
    State(state): State<Arc<AppState>>,
    auth: AdminAuth,
    headers: HeaderMap,
    Json(req): Json<AclUpdate>,
) -> Result<Json<AclView>, ApiErr> {
    auth.require_csrf(&headers)?;
    let nodes = db::list_nodes(&state.db).await?;
    let (mut policy, outcome) = acl::validate_yaml(&req.yaml, &nodes, state.overlay)
        .map_err(ApiErr::from)?;
    let current = db::get_acl_row(&state.db).await?;
    policy.version = current
        .map(|r| r.version as u32 + 1)
        .unwrap_or(1)
        .max(policy.version);
    let compiled = wm_proto::CompiledAcl {
        version: policy.version,
        ..outcome.compiled
    };
    db::store_acl(
        &state.db,
        &policy,
        &compiled,
        &format!("admin:{}", auth.username()),
    )
    .await?;
    db::audit(
        &state.db,
        &format!("admin:{}", auth.username()),
        "acl.update",
        "policy",
        Some(&format!("rules={}", compiled.rules.len())),
        client_ip(&headers).as_deref(),
    )
    .await
    .ok();
    state.bump_version();
    state.broadcast(ControlEvent::AclUpdated {
        version: compiled.version,
    });
    Ok(Json(AclView {
        policy,
        compiled,
        updated_at: Some(Utc::now()),
    }))
}

pub async fn validate_acl(
    State(state): State<Arc<AppState>>,
    _auth: AdminAuth,
    Json(req): Json<AclUpdate>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    let nodes = db::list_nodes(&state.db).await?;
    let (policy, outcome) = acl::validate_yaml(&req.yaml, &nodes, state.overlay)
        .map_err(ApiErr::from)?;
    Ok(Json(serde_json::json!({
        "ok": true,
        "policy": policy,
        "compiled": outcome.compiled,
        "warnings": outcome.warnings,
    })))
}

pub async fn simulate_acl(
    State(state): State<Arc<AppState>>,
    _auth: AdminAuth,
    Json(req): Json<SimulateRequest>,
) -> Result<Json<SimulateResponse>, ApiErr> {
    let nodes = db::list_nodes(&state.db).await?;
    let compiled = db::acl_view(&state.db).await?.compiled;
    Ok(Json(acl::simulate(&compiled, &nodes, &req).map_err(ApiErr::from)?))
}

#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
}

fn default_limit() -> i64 {
    200
}

pub async fn audit_log(
    State(state): State<Arc<AppState>>,
    _auth: AdminAuth,
    Query(q): Query<AuditQuery>,
) -> Result<Json<Vec<AuditEventView>>, ApiErr> {
    Ok(Json(
        db::list_audit(&state.db, q.limit.clamp(1, 1000)).await?,
    ))
}

pub async fn events(
    State(state): State<Arc<AppState>>,
    _auth: AdminAuth,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let rx = state.subscribe_events();
    let stream = stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let data = serde_json::to_string(&event).unwrap_or_default();
                    return Some((Ok(Event::default().event("wm").data(data)), rx));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

pub async fn list_service_tokens(
    State(state): State<Arc<AppState>>,
    _auth: AdminAuth,
) -> Result<Json<Vec<ServiceTokenView>>, ApiErr> {
    let rows = db::list_service_tokens(&state.db).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| {
                let scopes = r.scopes();
                ServiceTokenView {
                    id: Uuid::parse_str(&r.id).unwrap_or_else(|_| Uuid::new_v4()),
                    name: r.name,
                    scopes,
                    created_at: db::ts_to_dt(r.created_at),
                    last_used_at: db::opt_dt(r.last_used_at),
                    token: None,
                }
            })
            .collect(),
    ))
}

pub async fn create_service_token(
    State(state): State<Arc<AppState>>,
    auth: AdminAuth,
    headers: HeaderMap,
    Json(req): Json<CreateServiceTokenRequest>,
) -> Result<Json<ServiceTokenView>, ApiErr> {
    auth.require_csrf(&headers)?;
    if req.name.trim().is_empty() {
        return Err(ApiErr::bad_request("invalid_name", "nome do token inválido"));
    }
    let mut scopes = req.scopes;
    if scopes.is_empty() {
        scopes.push(SERVICE_SCOPE.to_string());
    }
    for scope in &scopes {
        if scope != SERVICE_SCOPE {
            return Err(ApiErr::bad_request(
                "invalid_scope",
                format!("escopo desconhecido: {scope}"),
            ));
        }
    }
    let token = auth::random_token("wms_")?;
    let row = db::create_service_token(
        &state.db,
        req.name.trim(),
        &auth::hash_token(&token),
        &scopes,
    )
    .await?;
    db::audit(
        &state.db,
        &format!("admin:{}", auth.username()),
        "service_token.create",
        &row.id,
        Some(req.name.trim()),
        client_ip(&headers).as_deref(),
    )
    .await
    .ok();
    let scopes = row.scopes();
    Ok(Json(ServiceTokenView {
        id: Uuid::parse_str(&row.id).unwrap_or_else(|_| Uuid::new_v4()),
        name: row.name,
        scopes,
        created_at: db::ts_to_dt(row.created_at),
        last_used_at: None,
        token: Some(token),
    }))
}

pub async fn revoke_service_token(
    State(state): State<Arc<AppState>>,
    auth: AdminAuth,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    auth.require_csrf(&headers)?;
    db::revoke_service_token(&state.db, id).await?;
    db::audit(
        &state.db,
        &format!("admin:{}", auth.username()),
        "service_token.revoke",
        &id.to_string(),
        None,
        client_ip(&headers).as_deref(),
    )
    .await
    .ok();
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn settings(
    State(state): State<Arc<AppState>>,
    _auth: AdminAuth,
) -> Json<SettingsView> {
    Json(SettingsView {
        network: state.network.clone(),
        approval_required: state.cfg.approval_required,
        relay_url: state
            .cfg
            .relay_enabled
            .then(|| state.cfg.relay_public_url()),
        stun: state.cfg.stun_urls.clone(),
        protocol: wm_proto::PROTOCOL_VERSION,
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

pub(super) async fn recompile_acl(state: &Arc<AppState>) -> anyhow::Result<()> {
    let Some(row) = db::get_acl_row(&state.db).await? else {
        return Ok(());
    };
    let mut policy: AclPolicy = serde_yaml::from_str(&row.yaml)?;
    let nodes = db::list_nodes(&state.db).await?;
    let outcome = crate::acl::compile_policy(&policy, &nodes, state.overlay)
        .map_err(|e| anyhow::anyhow!(e))?;
    policy.version = policy.version.max(row.version as u32 + 1);
    let compiled = wm_proto::CompiledAcl {
        version: policy.version,
        ..outcome.compiled
    };
    db::store_acl(&state.db, &policy, &compiled, "system:acl-recompile").await?;
    state.bump_version();
    state.broadcast(ControlEvent::AclUpdated {
        version: compiled.version,
    });
    Ok(())
}

#[allow(dead_code)]
fn _policy_default() -> AclPolicy {
    AclPolicy::default()
}
