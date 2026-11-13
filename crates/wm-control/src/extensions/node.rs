//! Node-authenticated extension catalog and activation.
//!
//! A member node owns its activation state: the TUI/local GUI can toggle an
//! extension without an admin session. The node token cannot change config or
//! see secrets; it can only turn the manifest on/off for itself.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use serde::Deserialize;

use wm_proto::{NodeExtensionView, NodeStatus};

use crate::api::{ApiErr, NodeAuth, client_ip};
use crate::db;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ActivationInput {
    pub active: bool,
}

pub async fn list(
    State(state): State<Arc<AppState>>,
    auth: NodeAuth,
) -> Result<Json<Vec<NodeExtensionView>>, ApiErr> {
    if auth.node.status() == NodeStatus::Disabled {
        return Err(ApiErr::forbidden("node_disabled", "nó revogado"));
    }
    Ok(Json(
        state
            .extensions
            .node_catalog(&state, &auth.node)
            .await?,
    ))
}

pub async fn set_active(
    State(state): State<Arc<AppState>>,
    auth: NodeAuth,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<ActivationInput>,
) -> Result<Json<NodeExtensionView>, ApiErr> {
    let actor = format!("node:{}", auth.node.name);
    let Some(view) = state
        .extensions
        .set_node_activation(&state, &auth.node, &id, input.active, &actor)
        .await?
    else {
        return Err(ApiErr::not_found(
            "extension_not_found",
            "extensão não encontrada",
        ));
    };
    db::audit(
        &state.db,
        &actor,
        if input.active {
            "extension.activate"
        } else {
            "extension.deactivate"
        },
        &format!("{id}/{}", auth.node.name),
        None,
        client_ip(&headers).as_deref(),
    )
    .await
    .ok();
    state.bump_version();
    Ok(Json(view))
}
