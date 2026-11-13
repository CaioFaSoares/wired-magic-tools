//! Declarative adapters: materialize manifest effects on the control plane.
//!
//! An adapter is intentionally small and idempotent. While the extension is
//! active on a node it:
//!
//! * merges declared labels into the node;
//! * creates/updates an owned share for each declared service;
//! * its `exposes` are projected into the map (reconciled by the agent).
//!
//! On deactivation adapter-owned shares are deleted and only labels whose
//! current value still matches the manifest are removed. A user-created share
//! is never claimed just because it has the same name: ownership is tracked in
//! `extension_adapter_resources`.

use std::collections::HashSet;

use wm_proto::NodeStatus;

use super::{ExtensionManifest, store};
use crate::db::{self, NodeRow};
use crate::state::AppState;

const SHARE_PREFIX: &str = "share:";

/// Applies all manifests to all nodes. Returns true when anything changed.
pub async fn sync_all(state: &AppState) -> anyhow::Result<bool> {
    let nodes = db::list_nodes(&state.db).await?;
    let manifests = state.extensions.manifests();
    let mut changed = false;
    for node in &nodes {
        for manifest in &manifests {
            if manifest.adapter.is_empty() {
                continue;
            }
            match sync_one(state, node, manifest).await {
                Ok(one) => changed |= one,
                Err(err) => tracing::warn!(
                    extension = %manifest.id,
                    node = %node.name,
                    %err,
                    "adapter sync falhou"
                ),
            }
        }
    }
    // Clean resources belonging to extensions that no longer exist.
    let known: HashSet<&str> = manifests.iter().map(|manifest| manifest.id.as_str()).collect();
    for row in store::list_all_adapter_resources(&state.db).await? {
        if known.contains(row.extension_id.as_str()) {
            continue;
        }
        let node_id = uuid_of(&row.node_id);
        if let Some(node) = db::node_by_id(&state.db, node_id).await? {
            changed |= remove_resource(state, &node, &row).await?;
        } else {
            store::delete_adapter_resource(
                &state.db,
                &row.extension_id,
                node_id,
                &row.resource,
            )
            .await?;
        }
    }
    Ok(changed)
}

/// Applies all manifests to one node. Used right after activation changes.
pub async fn sync_node(state: &AppState, node: &NodeRow) -> anyhow::Result<bool> {
    let mut changed = false;
    for manifest in state.extensions.manifests() {
        if manifest.adapter.is_empty() {
            continue;
        }
        match sync_one(state, node, &manifest).await {
            Ok(one) => changed |= one,
            Err(err) => tracing::warn!(
                extension = %manifest.id,
                node = %node.name,
                %err,
                "adapter sync falhou"
            ),
        }
    }
    Ok(changed)
}

async fn sync_one(
    state: &AppState,
    node: &NodeRow,
    manifest: &ExtensionManifest,
) -> anyhow::Result<bool> {
    // The activation handler may hold a pre-activation snapshot. Re-read the
    // node so deactivation can see (and remove) labels applied earlier.
    let fresh = db::node_by_id(&state.db, node.uuid()).await?;
    let node = fresh.as_ref().unwrap_or(node);
    // A revoked/pending node must not keep publishing labels or shares.
    let active = node.status() == NodeStatus::Active
        && state.extensions.is_active(state, manifest, node).await?;
    let mut changed = false;

    // --- labels -----------------------------------------------------------
    let mut labels = node.labels();
    let before = labels.clone();
    if active {
        for (key, value) in &manifest.adapter.labels {
            if labels.get(key) != Some(value) {
                labels.insert(key.clone(), value.clone());
            }
        }
    } else {
        for (key, value) in &manifest.adapter.labels {
            if labels.get(key) == Some(value) {
                labels.remove(key);
            }
        }
    }
    if labels != before {
        db::update_node(&state.db, node.uuid(), None, None, Some(&labels), None).await?;
        changed = true;
    }

    // --- shares -----------------------------------------------------------
    let mut current_keys = HashSet::new();
    for share in &manifest.adapter.shares {
        let name = match share.rendered_name(&manifest.id, &node.name) {
            Ok(name) => name,
            Err(err) => {
                tracing::warn!(extension = %manifest.id, node = %node.name, %err, "adapter share inválido");
                continue;
            }
        };
        let key = format!("{SHARE_PREFIX}{}", share.name);
        current_keys.insert(key.clone());
        let tracked = store::get_adapter_resource(
            &state.db,
            &manifest.id,
            node.uuid(),
            &key,
        )
        .await?;

        if active {
            match tracked {
                Some(row) => match db::share_by_id(&state.db, uuid_of(&row.resource_id)).await? {
                    Some(existing) if existing.node_uuid() == node.uuid() => {
                        if existing.name != name
                            || existing.port() != share.port
                            || existing.kind() != share.kind
                            || existing.protocol() != share.protocol
                            || existing.description != share.description
                            || !existing.enabled()
                        {
                            db::update_share(
                                &state.db,
                                existing.uuid(),
                                &name,
                                share.port,
                                share.kind,
                                share.protocol,
                                &share.description,
                                true,
                            )
                            .await?;
                            changed = true;
                        }
                    }
                    _ => {
                        store::delete_adapter_resource(
                            &state.db,
                            &manifest.id,
                            node.uuid(),
                            &key,
                        )
                        .await?;
                        if let Some(row) = create_share(state, node, manifest, share, &name, &key).await? {
                            let _ = row;
                            changed = true;
                        }
                    }
                },
                None => {
                    if db::share_by_name(&state.db, &name).await?.is_some() {
                        tracing::warn!(
                            extension = %manifest.id,
                            node = %node.name,
                            share = %name,
                            "adapter: já existe um share com este nome e ele não pertence ao adapter; mantendo o existente"
                        );
                    } else if create_share(state, node, manifest, share, &name, &key)
                        .await?
                        .is_some()
                    {
                        changed = true;
                    }
                }
            }
        } else if let Some(row) = tracked {
            changed |= remove_resource(state, node, &row).await?;
        }
    }

    // Remove adapter-owned shares whose declaration disappeared from the
    // manifest (e.g. an edit) or when the extension is inactive.
    for row in store::list_adapter_resources(&state.db, &manifest.id, node.uuid()).await? {
        if row.resource.starts_with(SHARE_PREFIX) && !current_keys.contains(&row.resource) {
            changed |= remove_resource(state, node, &row).await?;
        }
    }
    Ok(changed)
}

async fn create_share(
    state: &AppState,
    node: &NodeRow,
    manifest: &ExtensionManifest,
    share: &wm_ext::AdapterShare,
    name: &str,
    key: &str,
) -> anyhow::Result<Option<db::ShareRow>> {
    let row = db::create_share(
        &state.db,
        name,
        node.uuid(),
        share.port,
        share.kind,
        share.protocol,
        &share.description,
        true,
    )
    .await?;
    store::set_adapter_resource(
        &state.db,
        &manifest.id,
        node.uuid(),
        key,
        &row.id,
    )
    .await?;
    Ok(Some(row))
}

async fn remove_resource(
    state: &AppState,
    node: &NodeRow,
    row: &store::AdapterResourceRow,
) -> anyhow::Result<bool> {
    let Some(share) = db::share_by_id(&state.db, uuid_of(&row.resource_id)).await? else {
        store::delete_adapter_resource(&state.db, &row.extension_id, node.uuid(), &row.resource)
            .await?;
        return Ok(false);
    };
    let mut changed = false;
    if share.node_uuid() == node.uuid() {
        db::delete_share(&state.db, share.uuid()).await?;
        changed = true;
    }
    store::delete_adapter_resource(&state.db, &row.extension_id, node.uuid(), &row.resource).await?;
    Ok(changed)
}

fn uuid_of(raw: &str) -> uuid::Uuid {
    uuid::Uuid::parse_str(raw).unwrap_or_else(|_| uuid::Uuid::new_v4())
}
