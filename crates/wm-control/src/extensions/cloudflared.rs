//! Built-in cloudflared extension: control-side routes, node settings, desired
//! state and admin API.
//!
//! Two modes coexist on the same node:
//!
//! * `quick` — one ephemeral `cloudflared` process per route, giving a
//!   `*.trycloudflare.com` URL with no Cloudflare account or credentials.
//! * `named` — one process per node, with a locally rendered ingress config
//!   from the operator's credentials and hostnames.
//!
//! The agent reports each process state and quick URLs in
//! `ExtensionStatus.details.routes`; the panel merges that live state into the
//! route list.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use wm_ext::{
    CLOUDFLARED_ID, CloudflaredConfig, CloudflaredMode, CloudflaredRouteConfig,
    CloudflaredTunnelConfig, ExtensionManifest, validate_binary, validate_credentials,
    validate_hostname, validate_local_url, validate_project, validate_tunnel_id,
};
use wm_proto::{ExtensionDesired, ExtensionStatus, NodeStatus};

use super::store;
use super::{ControlExtension, ExtensionView};
use crate::api::{AdminAuth, ApiErr, client_ip};
use crate::db::{self, NodeRow};
use crate::state::AppState;

pub struct CloudflaredExtension;

#[async_trait]
impl ControlExtension for CloudflaredExtension {
    fn id(&self) -> &'static str {
        CLOUDFLARED_ID
    }

    fn manifest(&self) -> ExtensionManifest {
        wm_ext::builtin_cloudflared_manifest()
    }

    fn router(&self) -> Router<Arc<AppState>> {
        Router::new()
            .route(
                "/admin/extensions/cloudflared/routes",
                get(list_routes).post(create_route),
            )
            .route(
                "/admin/extensions/cloudflared/routes/{id}",
                put(update_route).delete(delete_route),
            )
            .route("/admin/extensions/cloudflared/nodes", get(list_nodes))
            .route(
                "/admin/extensions/cloudflared/overview",
                get(overview),
            )
            .route(
                "/admin/extensions/cloudflared/nodes/{node_id}",
                put(update_node_settings),
            )
            .route(
                "/admin/extensions/cloudflared/nodes/{node_id}/restart",
                post(restart_node),
            )
    }

    async fn desired_for(
        &self,
        state: &AppState,
        node: &NodeRow,
    ) -> anyhow::Result<Option<ExtensionDesired>> {
        let settings = store::get_node_settings(&state.db, node.uuid()).await?;
        let routes = store::list_routes_for_node(&state.db, node.uuid()).await?;
        let mut quick = Vec::new();
        let mut named = Vec::new();
        for row in routes.iter().filter(|r| r.enabled()) {
            let route = CloudflaredRouteConfig {
                id: row.id.clone(),
                project: row.project.clone(),
                local_url: row.local_url.clone(),
                hostname: row.hostname.clone(),
            };
            match row.mode() {
                CloudflaredMode::Quick => quick.push(route),
                CloudflaredMode::Named => named.push(route),
            }
        }
        let tunnel = settings.as_ref().and_then(|settings| {
            let tunnel_id = settings.tunnel_id.clone()?;
            let credentials_json = settings.credentials.clone()?;
            Some(CloudflaredTunnelConfig {
                tunnel_id,
                credentials_json,
            })
        });
        let config = CloudflaredConfig {
            binary: settings
                .as_ref()
                .map(|s| s.binary.clone())
                .unwrap_or_else(|| wm_ext::cloudflared::DEFAULT_BINARY.to_string()),
            tunnel,
            quick,
            named,
        };
        let enabled = config.route_count() > 0;
        let revision = settings.as_ref().map(|s| s.revision()).unwrap_or(0);
        Ok(Some(ExtensionDesired {
            id: CLOUDFLARED_ID.into(),
            name: self.manifest().name,
            driver: wm_ext::CLOUDFLARED_DRIVER.into(),
            revision,
            enabled,
            config: serde_json::to_value(&config)?,
        }))
    }
}

// ---------------------------------------------------------------------------
// API views
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct CloudflaredRouteView {
    pub id: Uuid,
    pub project: String,
    pub node_id: Uuid,
    pub node_name: String,
    pub node_online: bool,
    pub local_url: String,
    pub mode: CloudflaredMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<CloudflaredRouteLive>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CloudflaredRouteLive {
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CloudflaredNodeView {
    pub node_id: Uuid,
    pub node_name: String,
    pub hostname: String,
    pub overlay_ip: std::net::Ipv4Addr,
    pub online: bool,
    pub binary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_id: Option<String>,
    pub has_credentials: bool,
    pub revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<ExtensionStatus>,
    pub quick_routes: usize,
    pub named_routes: usize,
}

#[derive(Debug, Serialize)]
pub struct CloudflaredOverview {
    pub extension: ExtensionView,
    pub nodes: Vec<CloudflaredNodeView>,
    pub routes: Vec<CloudflaredRouteView>,
}

#[derive(Debug, Deserialize)]
pub struct RouteInput {
    pub project: String,
    pub node_id: Uuid,
    pub local_url: String,
    pub mode: CloudflaredMode,
    #[serde(default)]
    pub hostname: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub struct NodeSettingsInput {
    #[serde(default)]
    pub binary: Option<String>,
    /// `Some("")` clears the tunnel (and credentials).
    #[serde(default)]
    pub tunnel_id: Option<String>,
    /// `None` keeps the current secret; `Some("")` clears it.
    #[serde(default)]
    pub credentials_json: Option<String>,
}

struct ValidatedRoute {
    project: String,
    local_url: String,
    mode: CloudflaredMode,
    hostname: Option<String>,
    enabled: bool,
}

async fn validate_route(
    state: &AppState,
    input: &RouteInput,
    exclude: Option<Uuid>,
) -> Result<ValidatedRoute, ApiErr> {
    let project = validate_project(&input.project)
        .map_err(|err| ApiErr::bad_request("invalid_project", err))?;
    let local_url = validate_local_url(&input.local_url)
        .map_err(|err| ApiErr::bad_request("invalid_local_url", err))?;
    let Some(node) = db::node_by_id(&state.db, input.node_id).await? else {
        return Err(ApiErr::not_found("node_not_found", "nó não encontrado"));
    };
    if node.status() == NodeStatus::Disabled {
        return Err(ApiErr::conflict(
            "node_disabled",
            "o nó está desativado; reative-o antes de criar rotas",
        ));
    }
    let hostname = match input.mode {
        CloudflaredMode::Quick => None,
        CloudflaredMode::Named => {
            let raw = input
                .hostname
                .as_deref()
                .filter(|h| !h.trim().is_empty())
                .ok_or_else(|| {
                    ApiErr::bad_request(
                        "hostname_required",
                        "rotas named exigem um hostname público",
                    )
                })?;
            Some(validate_hostname(raw).map_err(|err| ApiErr::bad_request("invalid_hostname", err))?)
        }
    };

    if let Some(hostname) = hostname.as_deref() {
        let routes = store::list_routes(&state.db).await?;
        if let Some(conflict) = routes.iter().find(|r| {
            Some(r.uuid()) != exclude
                && r.hostname.as_deref().map(str::to_ascii_lowercase)
                    == Some(hostname.to_ascii_lowercase())
        }) {
            return Err(ApiErr::conflict(
                "hostname_taken",
                format!(
                    "hostname '{}' já é usado pela rota '{}'",
                    hostname, conflict.project
                ),
            ));
        }
    }

    if input.mode == CloudflaredMode::Named {
        let settings = store::get_node_settings(&state.db, input.node_id).await?;
        let ready = settings
            .as_ref()
            .map(|s| s.tunnel_id.is_some() && s.has_credentials())
            .unwrap_or(false);
        if !ready {
            return Err(ApiErr::bad_request(
                "tunnel_not_configured",
                "configure o tunnel_id e as credenciais do cloudflared para este nó antes de criar rotas named",
            ));
        }
    }

    let existing = store::list_routes_for_node(&state.db, input.node_id).await?;
    let count = existing
        .iter()
        .filter(|r| Some(r.uuid()) != exclude)
        .count();
    if count >= wm_ext::cloudflared::MAX_ROUTES {
        return Err(ApiErr::bad_request(
            "too_many_routes",
            format!("no máximo {} rotas por nó", wm_ext::cloudflared::MAX_ROUTES),
        ));
    }

    Ok(ValidatedRoute {
        project,
        local_url,
        mode: input.mode,
        hostname,
        enabled: input.enabled,
    })
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

pub async fn list_routes(
    State(state): State<Arc<AppState>>,
    _auth: AdminAuth,
) -> Result<Json<Vec<CloudflaredRouteView>>, ApiErr> {
    let rows = store::list_routes(&state.db).await?;
    Ok(Json(build_route_views(&state, rows).await?))
}

pub async fn create_route(
    State(state): State<Arc<AppState>>,
    auth: AdminAuth,
    headers: HeaderMap,
    Json(input): Json<RouteInput>,
) -> Result<Json<CloudflaredRouteView>, ApiErr> {
    auth.require_csrf(&headers)?;
    let route = validate_route(&state, &input, None).await?;
    let row = store::create_route(
        &state.db,
        &store::NewCloudflaredRoute {
            project: &route.project,
            node_id: input.node_id,
            local_url: &route.local_url,
            mode: route.mode,
            hostname: route.hostname.as_deref(),
            enabled: route.enabled,
        },
    )
    .await?;
    db::audit(
        &state.db,
        &format!("admin:{}", auth.username()),
        "cloudflared.route_create",
        &row.project,
        Some(&format!("node={} mode={}", row.node_id, row.mode)),
        client_ip(&headers).as_deref(),
    )
    .await
    .ok();
    state.bump_version();
    let mut views = build_route_views(&state, vec![row]).await?;
    views
        .pop()
        .map(Json)
        .ok_or_else(|| ApiErr::internal("rota não encontrada após criação"))
}

pub async fn update_route(
    State(state): State<Arc<AppState>>,
    auth: AdminAuth,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<RouteInput>,
) -> Result<Json<CloudflaredRouteView>, ApiErr> {
    auth.require_csrf(&headers)?;
    if store::get_route(&state.db, id).await?.is_none() {
        return Err(ApiErr::not_found("route_not_found", "rota não encontrada"));
    }
    let route = validate_route(&state, &input, Some(id)).await?;
    let row = store::update_route(
        &state.db,
        id,
        &store::NewCloudflaredRoute {
            project: &route.project,
            node_id: input.node_id,
            local_url: &route.local_url,
            mode: route.mode,
            hostname: route.hostname.as_deref(),
            enabled: route.enabled,
        },
    )
    .await
    .map_err(|err| ApiErr::from(err))?;
    db::audit(
        &state.db,
        &format!("admin:{}", auth.username()),
        "cloudflared.route_update",
        &row.project,
        Some(&format!("node={} enabled={}", row.node_id, row.enabled())),
        client_ip(&headers).as_deref(),
    )
    .await
    .ok();
    state.bump_version();
    let mut views = build_route_views(&state, vec![row]).await?;
    views
        .pop()
        .map(Json)
        .ok_or_else(|| ApiErr::internal("rota não encontrada após atualização"))
}

pub async fn delete_route(
    State(state): State<Arc<AppState>>,
    auth: AdminAuth,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    auth.require_csrf(&headers)?;
    let Some(row) = store::delete_route(&state.db, id).await? else {
        return Err(ApiErr::not_found("route_not_found", "rota não encontrada"));
    };
    db::audit(
        &state.db,
        &format!("admin:{}", auth.username()),
        "cloudflared.route_delete",
        &row.project,
        Some(&format!("node={} mode={}", row.node_id, row.mode)),
        client_ip(&headers).as_deref(),
    )
    .await
    .ok();
    state.bump_version();
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn build_route_views(
    state: &AppState,
    rows: Vec<store::CloudflaredRouteRow>,
) -> Result<Vec<CloudflaredRouteView>, ApiErr> {
    let nodes: HashMap<String, NodeRow> = db::list_nodes(&state.db)
        .await?
        .into_iter()
        .map(|n| (n.id.clone(), n))
        .collect();
    let statuses: HashMap<String, store::ExtensionStatusRow> =
        store::list_statuses(&state.db, CLOUDFLARED_ID)
            .await?
            .into_iter()
            .map(|s| (s.node_id.clone(), s))
            .collect();
    let now = db::now_ts();
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let node = nodes.get(&row.node_id);
        let live = statuses.get(&row.node_id).and_then(|status| {
            let mut map = live_routes(status);
            map.remove(&row.id)
        });
        let status = live.or_else(|| {
            (row.mode() == CloudflaredMode::Named && row.enabled()).then(|| {
                CloudflaredRouteLive {
                    state: "pending".into(),
                    public_url: row.hostname.as_ref().map(|h| format!("https://{h}")),
                    message: node
                        .filter(|n| !n.online(now) && !state.relay.is_online(n.uuid()))
                        .map(|_| "nó offline".into()),
                }
            })
        });
        out.push(CloudflaredRouteView {
            id: row.uuid(),
            project: row.project.clone(),
            node_id: row.node_uuid(),
            node_name: node.map(|n| n.name.clone()).unwrap_or_else(|| "?".into()),
            node_online: node
                .map(|n| n.online(now) || state.relay.is_online(n.uuid()))
                .unwrap_or(false),
            local_url: row.local_url.clone(),
            mode: row.mode(),
            hostname: row.hostname.clone(),
            enabled: row.enabled(),
            created_at: row.created_at(),
            updated_at: row.updated_at(),
            status: if row.enabled() { status } else { None },
        });
    }
    out.sort_by(|a, b| a.project.cmp(&b.project).then(a.id.cmp(&b.id)));
    Ok(out)
}

fn live_routes(status: &store::ExtensionStatusRow) -> HashMap<String, CloudflaredRouteLive> {
    let view = status.view();
    let mut out = HashMap::new();
    let Some(routes) = view.details.get("routes").and_then(|v| v.as_array()) else {
        return out;
    };
    for route in routes {
        let Some(id) = route.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        out.insert(
            id.to_string(),
            CloudflaredRouteLive {
                state: route
                    .get("state")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                public_url: route
                    .get("public_url")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                message: route
                    .get("message")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            },
        );
    }
    out
}

// ---------------------------------------------------------------------------
// Node settings
// ---------------------------------------------------------------------------

pub async fn list_nodes(
    State(state): State<Arc<AppState>>,
    _auth: AdminAuth,
) -> Result<Json<Vec<CloudflaredNodeView>>, ApiErr> {
    Ok(Json(build_node_views(&state).await?))
}

async fn build_node_views(state: &AppState) -> Result<Vec<CloudflaredNodeView>, ApiErr> {
    let nodes = db::list_nodes(&state.db).await?;
    let settings = store::list_node_settings(&state.db).await?;
    let routes = store::list_routes(&state.db).await?;
    let statuses: HashMap<String, store::ExtensionStatusRow> =
        store::list_statuses(&state.db, CLOUDFLARED_ID)
            .await?
            .into_iter()
            .map(|s| (s.node_id.clone(), s))
            .collect();
    let now = db::now_ts();
    let mut out = Vec::with_capacity(nodes.len());
    for node in nodes {
        let id = node.id.clone();
        let setting = settings.iter().find(|s| s.node_id == id);
        let node_routes: Vec<&store::CloudflaredRouteRow> =
            routes.iter().filter(|r| r.node_id == id).collect();
        out.push(CloudflaredNodeView {
            node_id: node.uuid(),
            node_name: node.name.clone(),
            hostname: node.hostname.clone(),
            overlay_ip: node.overlay_ip(),
            online: node.online(now) || state.relay.is_online(node.uuid()),
            binary: setting
                .map(|s| s.binary.clone())
                .unwrap_or_else(|| wm_ext::cloudflared::DEFAULT_BINARY.into()),
            tunnel_id: setting.and_then(|s| s.tunnel_id.clone()),
            has_credentials: setting.map(|s| s.has_credentials()).unwrap_or(false),
            revision: setting.map(|s| s.revision()).unwrap_or(0),
            updated_at: setting.map(|s| db::ts_to_dt(s.updated_at)),
            status: statuses.get(&id).map(|s| s.view()),
            quick_routes: node_routes
                .iter()
                .filter(|r| r.mode() == CloudflaredMode::Quick)
                .count(),
            named_routes: node_routes
                .iter()
                .filter(|r| r.mode() == CloudflaredMode::Named)
                .count(),
        });
    }
    out.sort_by(|a, b| a.node_name.cmp(&b.node_name));
    Ok(out)
}

pub async fn update_node_settings(
    State(state): State<Arc<AppState>>,
    auth: AdminAuth,
    headers: HeaderMap,
    Path(node_id): Path<Uuid>,
    Json(input): Json<NodeSettingsInput>,
) -> Result<Json<CloudflaredNodeView>, ApiErr> {
    auth.require_csrf(&headers)?;
    let Some(node) = db::node_by_id(&state.db, node_id).await? else {
        return Err(ApiErr::not_found("node_not_found", "nó não encontrado"));
    };
    let existing = store::get_node_settings(&state.db, node_id).await?;
    let binary = match input.binary.as_deref() {
        Some(raw) if raw.trim().is_empty() => wm_ext::cloudflared::DEFAULT_BINARY.to_string(),
        Some(raw) => validate_binary(raw).map_err(|err| ApiErr::bad_request("invalid_binary", err))?,
        None => existing
            .as_ref()
            .map(|s| s.binary.clone())
            .unwrap_or_else(|| wm_ext::cloudflared::DEFAULT_BINARY.into()),
    };
    let tunnel_id = match input.tunnel_id.as_deref() {
        Some(raw) if raw.trim().is_empty() => None,
        Some(raw) => {
            Some(validate_tunnel_id(raw).map_err(|err| ApiErr::bad_request("invalid_tunnel_id", err))?)
        }
        None => existing.as_ref().and_then(|s| s.tunnel_id.clone()),
    };
    let credentials = if tunnel_id.is_none() {
        None
    } else {
        match input.credentials_json.as_deref() {
            Some(raw) if raw.trim().is_empty() => None,
            Some(raw) => {
                validate_credentials(raw, tunnel_id.as_deref().unwrap_or_default())
                    .map_err(|err| ApiErr::bad_request("invalid_credentials", err))?;
                Some(raw.trim().to_string())
            }
            None => existing.as_ref().and_then(|s| s.credentials.clone()),
        }
    };
    if tunnel_id.is_some() && credentials.is_none() {
        return Err(ApiErr::bad_request(
            "credentials_required",
            "informe o credentials_json do túnel cloudflared",
        ));
    }
    // A credentials file is bound to one tunnel. If the operator rotates the
    // tunnel ID but keeps the old secret, fail here instead of letting the
    // node report a confusing cloudflared error.
    if let (Some(tunnel_id), Some(credentials)) = (&tunnel_id, &credentials) {
        validate_credentials(credentials, tunnel_id)
            .map_err(|err| ApiErr::bad_request("invalid_credentials", err))?;
    }
    if tunnel_id.is_none() {
        let named = store::list_routes_for_node(&state.db, node_id)
            .await?
            .into_iter()
            .filter(|r| r.enabled() && r.mode() == CloudflaredMode::Named)
            .count();
        if named > 0 {
            return Err(ApiErr::conflict(
                "named_routes_active",
                "há rotas named ativas; remova-as ou desative-as antes de limpar o túnel",
            ));
        }
    }
    store::upsert_node_settings(
        &state.db,
        node_id,
        &store::CloudflaredNodeSettings {
            binary: &binary,
            tunnel_id: tunnel_id.as_deref(),
            credentials: credentials.as_deref(),
        },
    )
    .await?;
    db::audit(
        &state.db,
        &format!("admin:{}", auth.username()),
        "cloudflared.node_settings",
        &node.name,
        Some(&format!(
            "binary={binary} tunnel={} has_credentials={}",
            tunnel_id.is_some(),
            credentials.is_some()
        )),
        client_ip(&headers).as_deref(),
    )
    .await
    .ok();
    state.bump_version();
    let views = build_node_views(&state).await?;
    views
        .into_iter()
        .find(|v| v.node_id == node_id)
        .map(Json)
        .ok_or_else(|| ApiErr::not_found("node_not_found", "nó não encontrado"))
}

pub async fn restart_node(
    State(state): State<Arc<AppState>>,
    auth: AdminAuth,
    headers: HeaderMap,
    Path(node_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    auth.require_csrf(&headers)?;
    let Some(node) = db::node_by_id(&state.db, node_id).await? else {
        return Err(ApiErr::not_found("node_not_found", "nó não encontrado"));
    };
    store::bump_node_revision(&state.db, node_id).await?;
    db::audit(
        &state.db,
        &format!("admin:{}", auth.username()),
        "cloudflared.restart",
        &node.name,
        None,
        client_ip(&headers).as_deref(),
    )
    .await
    .ok();
    state.bump_version();
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// Convenience for the panel: one request instead of three.
pub async fn overview(
    State(state): State<Arc<AppState>>,
    _auth: AdminAuth,
) -> Result<Json<CloudflaredOverview>, ApiErr> {
    let manifest = wm_ext::builtin_cloudflared_manifest();
    let extension = state.extensions.view_for(&state, &manifest).await?;
    Ok(Json(CloudflaredOverview {
        extension,
        nodes: build_node_views(&state).await?,
        routes: build_route_views(&state, store::list_routes(&state.db).await?).await?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_routes_reads_details() {
        let status = store::ExtensionStatusRow {
            extension_id: CLOUDFLARED_ID.into(),
            node_id: Uuid::new_v4().to_string(),
            name: "Cloudflare Tunnel".into(),
            driver: "cloudflared".into(),
            state: "running".into(),
            message: None,
            public_urls: "[]".into(),
            details: serde_json::json!({
                "routes": [
                    {"id": "a", "state": "running", "public_url": "https://x.trycloudflare.com"},
                    {"id": "b", "state": "error", "message": "boom"}
                ]
            })
            .to_string(),
            updated_at: 0,
        };
        let live = live_routes(&status);
        assert_eq!(live["a"].state, "running");
        assert_eq!(
            live["a"].public_url.as_deref(),
            Some("https://x.trycloudflare.com")
        );
        assert_eq!(live["b"].message.as_deref(), Some("boom"));
    }
}
