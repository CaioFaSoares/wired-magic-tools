use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::Deserialize;

use wm_proto::{IntegrationNode, IntegrationNodesResponse};

use super::{ApiErr, ServiceAuth};
use crate::db;
use crate::state::AppState;

const READ_SCOPE: &str = "integration:nodes:read";

#[derive(Debug, Deserialize)]
pub struct Filter {
    pub tag: Option<String>,
    pub online: Option<bool>,
}

pub async fn nodes(
    State(state): State<Arc<AppState>>,
    auth: ServiceAuth,
    Query(filter): Query<Filter>,
) -> Result<Json<IntegrationNodesResponse>, ApiErr> {
    auth.require_scope(READ_SCOPE)?;
    let now = db::now_ts();
    let mut nodes = Vec::new();
    for row in db::list_nodes(&state.db).await? {
        if row.status() != wm_proto::NodeStatus::Active {
            continue;
        }
        let online = row.online(now) || state.relay.is_online(row.uuid());
        if let Some(tag) = &filter.tag {
            if !row.tags().iter().any(|t| t == tag) {
                continue;
            }
        }
        if let Some(want) = filter.online {
            if want != online {
                continue;
            }
        }
        nodes.push(IntegrationNode {
            id: row.uuid(),
            name: row.name.clone(),
            hostname: row.hostname.clone(),
            overlay_ip: row.overlay_ip(),
            tags: row.tags(),
            labels: row.labels(),
            online,
            last_seen: db::opt_dt(row.last_seen_at),
        });
    }
    Ok(Json(IntegrationNodesResponse {
        nodes,
        server_time: chrono::Utc::now(),
    }))
}

pub async fn node(
    State(state): State<Arc<AppState>>,
    auth: ServiceAuth,
    Path(name): Path<String>,
) -> Result<Json<IntegrationNode>, ApiErr> {
    auth.require_scope(READ_SCOPE)?;
    let row = db::node_by_name(&state.db, &name)
        .await?
        .ok_or_else(|| ApiErr::not_found("node_not_found", "nó não encontrado"))?;
    if row.status() != wm_proto::NodeStatus::Active {
        return Err(ApiErr::not_found("node_not_found", "nó não encontrado"));
    }
    let now = db::now_ts();
    let online = row.online(now) || state.relay.is_online(row.uuid());
    Ok(Json(IntegrationNode {
        id: row.uuid(),
        name: row.name.clone(),
        hostname: row.hostname.clone(),
        overlay_ip: row.overlay_ip(),
        tags: row.tags(),
        labels: row.labels(),
        online,
        last_seen: db::opt_dt(row.last_seen_at),
    }))
}

pub async fn health(
    State(state): State<Arc<AppState>>,
    auth: ServiceAuth,
) -> Result<Json<serde_json::Value>, ApiErr> {
    auth.require_scope(READ_SCOPE)?;
    let now = db::now_ts();
    let nodes = db::list_nodes(&state.db).await?;
    let online = nodes
        .iter()
        .filter(|n| n.status() == wm_proto::NodeStatus::Active)
        .filter(|n| n.online(now) || state.relay.is_online(n.uuid()))
        .count();
    Ok(Json(serde_json::json!({
        "ok": true,
        "nodes_total": nodes.len(),
        "nodes_online": online,
        "server_time": chrono::Utc::now(),
    })))
}
