//! Extension host for the control plane.
//!
//! Extensions have two independent halves:
//!
//! * **control side** — a manifest (on disk), optional code-backed handlers and
//!   database tables. It produces the desired state for each node.
//! * **node side** — a sidecar supervised by `wm-core`, selected by `driver`.
//!
//! The host deliberately keeps the wire path simple: desired state rides the
//! existing `NetworkMap` long-poll and observed state rides `ReportRequest`.
//! There is no new port, token or connection to operate.
//!
//! ## Creating an extension
//!
//! * Declarative (no Rust): create `<data_dir>/extensions/<id>/extension.toml`
//!   with `node_driver = "exec"` and configure a command per node through the
//!   panel. The agent supervises it.
//! * Code-backed: implement [`ControlExtension`], register it in
//!   [`ExtensionRegistry::load`] and implement a driver under
//!   `wm-core/src/extensions`. `cloudflared` is the reference implementation.

pub mod adapters;
pub mod cloudflared;
pub mod control_panel;
pub mod node;
pub mod store;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use axum::Json;
use axum::extract::{Path as AxumPath, State};
use axum::routing::{get, post, put};
use axum::Router;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use wm_ext::{
    CLOUDFLARED_ID, ExtensionManifest, NodeDriver, load_dir, seed_builtin_manifests,
};
use wm_proto::{ExtensionDesired, ExtensionStatus, NodeExtensionView};

use crate::api::{AdminAuth, ApiErr, client_ip};
use crate::db::{self, NodeRow};
use crate::state::AppState;

/// A code-backed control-side extension.
#[async_trait]
pub trait ControlExtension: Send + Sync {
    fn id(&self) -> &'static str;
    fn manifest(&self) -> ExtensionManifest;
    /// Extension-owned admin routes, mounted under `/api/v1`.
    fn router(&self) -> Router<Arc<AppState>>;
    /// Desired state for one node, if the extension applies.
    async fn desired_for(
        &self,
        state: &AppState,
        node: &NodeRow,
    ) -> anyhow::Result<Option<ExtensionDesired>>;
}

pub struct ExtensionRegistry {
    root: PathBuf,
    manifests: RwLock<BTreeMap<String, ExtensionManifest>>,
    code: BTreeMap<String, Arc<dyn ControlExtension>>,
    warnings: RwLock<Vec<String>>,
    pub(crate) panel: Arc<control_panel::PanelRuntime>,
}

impl ExtensionRegistry {
    /// Loads manifests from `<data_dir>/extensions`, seeding the built-in ones.
    /// A manifest problem never prevents boot: it becomes a warning and the
    /// extension is skipped (except cloudflared, which falls back to its
    /// embedded manifest so the host API stays reachable).
    pub fn load(data_dir: &Path) -> anyhow::Result<Self> {
        let root = data_dir.join("extensions");
        seed_builtin_manifests(&root)?;
        let loaded = load_dir(&root);
        let mut warnings = loaded.warnings;
        let mut manifests = BTreeMap::new();
        for manifest in loaded.manifests {
            manifests.insert(manifest.id.clone(), manifest);
        }
        if !manifests.contains_key(CLOUDFLARED_ID) {
            warnings.push(
                "manifesto do cloudflared ausente ou inválido; usando o embutido".into(),
            );
            let builtin = wm_ext::builtin_cloudflared_manifest();
            manifests.insert(builtin.id.clone(), builtin);
        }
        let cloudflared: Arc<dyn ControlExtension> = Arc::new(cloudflared::CloudflaredExtension);
        let mut code: BTreeMap<String, Arc<dyn ControlExtension>> = BTreeMap::new();
        code.insert(cloudflared.id().to_string(), cloudflared);
        let panel: Arc<dyn ControlExtension> = Arc::new(control_panel::PanelExtension);
        manifests.insert(panel.id().to_string(), panel.manifest());
        code.insert(panel.id().to_string(), panel);
        for warning in &warnings {
            tracing::warn!(%warning, "extensão ignorada no carregamento");
        }
        Ok(Self {
            root,
            manifests: RwLock::new(manifests),
            code,
            warnings: RwLock::new(warnings),
            panel: Arc::new(control_panel::PanelRuntime::load(data_dir)),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn warnings(&self) -> Vec<String> {
        self.warnings
            .read()
            .map(|w| w.clone())
            .unwrap_or_default()
    }

    pub fn manifests(&self) -> Vec<ExtensionManifest> {
        self.manifests
            .read()
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default()
    }

    pub fn manifest(&self, id: &str) -> Option<ExtensionManifest> {
        self.manifests.read().ok()?.get(id).cloned()
    }

    pub fn code_extensions(&self) -> Vec<Arc<dyn ControlExtension>> {
        self.code.values().cloned().collect()
    }

    pub fn is_builtin(&self, id: &str) -> bool {
        self.code.contains_key(id)
    }

    /// Re-reads the manifest directory without restarting the control plane.
    /// Code-backed handlers are not hot-swapped; only metadata/config change.
    pub fn reload(&self) -> anyhow::Result<Vec<String>> {
        seed_builtin_manifests(&self.root)?;
        let loaded = load_dir(&self.root);
        let mut warnings = loaded.warnings;
        let mut manifests = BTreeMap::new();
        for manifest in loaded.manifests {
            manifests.insert(manifest.id.clone(), manifest);
        }
        if !manifests.contains_key(CLOUDFLARED_ID) {
            warnings.push(
                "manifesto do cloudflared ausente ou inválido; usando o embutido".into(),
            );
            let builtin = wm_ext::builtin_cloudflared_manifest();
            manifests.insert(builtin.id.clone(), builtin);
        }
        // Server-only runtime remains registered even if no asset package is
        // installed. Reloading metadata never activates or refreshes its code.
        let panel = control_panel::PanelExtension;
        manifests.insert(panel.id().to_string(), panel.manifest());
        if let Ok(mut current) = self.manifests.write() {
            *current = manifests;
        }
        if let Ok(mut current) = self.warnings.write() {
            *current = warnings.clone();
        }
        Ok(warnings)
    }

    /// Builds the authoritative desired-state list for one node. Errors from a
    /// single extension are logged and do not fail the map (a node must keep
    /// its data plane even if an extension database is unhappy).
    pub async fn desired_for(&self, state: &AppState, node: &NodeRow) -> Vec<ExtensionDesired> {
        let manifests = self.manifests();
        let mut out = Vec::new();
        for manifest in manifests {
            let active = match self.is_active(state, &manifest, node).await {
                Ok(active) => active,
                Err(err) => {
                    tracing::warn!(extension = %manifest.id, %err, "falha ao ler ativação");
                    continue;
                }
            };
            if !active {
                continue;
            }
            let activation_revision = self
                .activation_revision(state, &manifest.id, node)
                .await
                .unwrap_or(0);
            let manager = self.code.get(&manifest.id).cloned();
            let desired = match manifest.node_driver {
                NodeDriver::Cloudflared => match &manager {
                    Some(manager) => manager.desired_for(state, node).await,
                    None => Ok(None),
                },
                NodeDriver::Exec => generic_desired_for(state, node, &manifest).await,
                NodeDriver::None => Ok(None),
            };
            match desired {
                Ok(Some(mut desired)) => {
                    desired.version = manifest.version.clone();
                    desired.activation_revision = activation_revision;
                    desired.hooks = manifest.hooks.clone();
                    out.push(desired);
                }
                Ok(None) => {}
                Err(err) => {
                    tracing::warn!(extension = %manifest.id, %err, "falha ao montar estado desejado");
                }
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// Effective activation for one node. An absent row falls back to the
    /// pre-activation behavior so an existing deployment does not lose routes
    /// or exec sidecars when it upgrades.
    pub async fn is_active(
        &self,
        state: &AppState,
        manifest: &ExtensionManifest,
        node: &NodeRow,
    ) -> anyhow::Result<bool> {
        if manifest.id == control_panel::ID {
            return Ok(false);
        }
        if let Some(row) = store::get_activation(&state.db, &manifest.id, node.uuid()).await? {
            return Ok(row.enabled());
        }
        match manifest.node_driver {
            NodeDriver::Cloudflared => cloudflared::has_config(state, node).await,
            NodeDriver::Exec => {
                if let Some(record) = store::get_record(&state.db, &manifest.id, node.uuid()).await? {
                    return Ok(record.enabled());
                }
                Ok(manifest.default_active)
            }
            NodeDriver::None => Ok(manifest.default_active),
        }
    }

    pub async fn activation_revision(
        &self,
        state: &AppState,
        extension_id: &str,
        node: &NodeRow,
    ) -> anyhow::Result<u64> {
        Ok(store::get_activation(&state.db, extension_id, node.uuid())
            .await?
            .map(|row| row.revision())
            .unwrap_or(0))
    }

    /// Catalog used by the TUI/local GUI: one row per manifest for this node.
    pub async fn node_catalog(
        &self,
        state: &AppState,
        node: &NodeRow,
    ) -> anyhow::Result<Vec<NodeExtensionView>> {
        let statuses: BTreeMap<String, ExtensionStatus> =
            store::list_statuses_for_node(&state.db, node.uuid())
                .await?
                .into_iter()
                .map(|row| (row.extension_id.clone(), row.view()))
                .collect();
        let requests = store::requests_for_node(&state.db, node.uuid()).await?;
        let mut out = Vec::new();
        for manifest in self.manifests() {
            if manifest.id == control_panel::ID {
                continue; // Server application, never a node-side install/action.
            }
            let active = self.is_active(state, &manifest, node).await?;
            let configured = match manifest.node_driver {
                NodeDriver::Cloudflared => cloudflared::has_config(state, node).await?,
                NodeDriver::Exec => store::get_record(&state.db, &manifest.id, node.uuid())
                    .await?
                    .is_some(),
                NodeDriver::None => false,
            };
            let activation_revision = self.activation_revision(state, &manifest.id, node).await?;
            out.push(NodeExtensionView {
                id: manifest.id.clone(),
                name: manifest.name.clone(),
                version: manifest.version.clone(),
                description: manifest.description.clone(),
                driver: manifest.node_driver.as_str().to_string(),
                ui: manifest.ui_kind().to_string(),
                builtin: self.is_builtin(&manifest.id),
                active,
                configured,
                activation_revision,
                default_config: !manifest.default_config.is_null(),
                requested: requests.contains_key(&manifest.id),
                request_message: requests.get(&manifest.id).cloned(),
                tui: manifest.tui.clone(),
                status: statuses.get(&manifest.id).cloned(),
            });
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    /// Activates or deactivates an extension for one node. When activating a
    /// declarative extension without explicit config, `default_config` is
    /// materialized so "ativar" is enough to bootstrap the sidecar.
    pub async fn set_node_activation(
        &self,
        state: &AppState,
        node: &NodeRow,
        extension_id: &str,
        active: bool,
        actor: &str,
    ) -> anyhow::Result<Option<NodeExtensionView>> {
        if extension_id == control_panel::ID {
            return Ok(None); // Runtime API, not an activation record per node.
        }
        let Some(manifest) = self.manifest(extension_id) else {
            return Ok(None);
        };
        if active && manifest.node_driver == NodeDriver::Exec {
            let existing = store::get_record(&state.db, extension_id, node.uuid()).await?;
            match existing {
                Some(record) if !record.enabled() => {
                    let config = record.config_value();
                    validate_generic_config(&manifest, &config)
                        .map_err(|err| anyhow::anyhow!("{}", err.message))?;
                    store::upsert_record(
                        &state.db,
                        extension_id,
                        node.uuid(),
                        true,
                        &config,
                        actor,
                    )
                    .await?;
                }
                Some(_) => {}
                None if !manifest.default_config.is_null() => {
                    validate_generic_config(&manifest, &manifest.default_config)
                        .map_err(|err| anyhow::anyhow!("{}", err.message))?;
                    store::upsert_record(
                        &state.db,
                        extension_id,
                        node.uuid(),
                        true,
                        &manifest.default_config,
                        actor,
                    )
                    .await?;
                }
                None => {}
            }
        }
        store::set_activation(&state.db, extension_id, node.uuid(), active, actor).await?;
        if !active {
            // Do not show a stale running state for a deactivated extension.
            store::delete_status(&state.db, extension_id, node.uuid()).await?;
        }
        match adapters::sync_node(state, node).await {
            Ok(true) => state.bump_version(),
            Ok(false) => {}
            Err(err) => tracing::warn!(extension = %extension_id, %err, "adapter sync falhou"),
        }
        let catalog = self.node_catalog(state, node).await?;
        Ok(catalog.into_iter().find(|view| view.id == extension_id))
    }

    /// Exposes requested by active adapters for this node.
    pub async fn exposes_for(&self, state: &AppState, node: &NodeRow) -> Vec<wm_proto::MeshExpose> {
        if node.status() != wm_proto::NodeStatus::Active {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for manifest in self.manifests() {
            if manifest.adapter.exposes.is_empty() {
                continue;
            }
            let active = self.is_active(state, &manifest, node).await.unwrap_or(false);
            if !active {
                continue;
            }
            for expose in &manifest.adapter.exposes {
                let mesh: wm_proto::MeshExpose = (*expose).into();
                if seen.insert(mesh.overlay_port) {
                    out.push(mesh);
                }
            }
        }
        out.sort_by_key(|expose| expose.overlay_port);
        out
    }

    pub async fn views(&self, state: &AppState) -> anyhow::Result<Vec<ExtensionView>> {
        let mut out = Vec::new();
        for manifest in self.manifests() {
            out.push(self.view_for(state, &manifest).await?);
        }
        Ok(out)
    }

    pub async fn view_for(
        &self,
        state: &AppState,
        manifest: &ExtensionManifest,
    ) -> anyhow::Result<ExtensionView> {
        let statuses = store::list_statuses(&state.db, &manifest.id).await?;
        let running = statuses.iter().filter(|s| s.online()).count();
        let (configured, warning) = match manifest.node_driver {
            NodeDriver::Cloudflared => {
                let routes = store::list_routes(&state.db).await?;
                let nodes: std::collections::HashSet<String> =
                    routes.iter().map(|r| r.node_id.clone()).collect();
                (nodes.len(), None)
            }
            NodeDriver::Exec => {
                let records = store::list_records(&state.db, &manifest.id).await?;
                (records.iter().filter(|r| r.enabled()).count(), None)
            }
            NodeDriver::None => (0, None),
        };
        Ok(ExtensionView {
            id: manifest.id.clone(),
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            description: manifest.description.clone(),
            author: manifest.author.clone(),
            homepage: manifest.homepage.clone(),
            builtin: self.is_builtin(&manifest.id),
            visibility: manifest.visibility.as_str().to_string(),
            node_driver: manifest.node_driver.as_str().to_string(),
            ui: manifest.ui_kind().to_string(),
            config_example: manifest.config_example.clone(),
            nodes_configured: configured,
            nodes_running: running,
            warning,
        })
    }

    pub async fn detail(
        &self,
        state: &AppState,
        id: &str,
    ) -> anyhow::Result<Option<ExtensionDetail>> {
        let Some(manifest) = self.manifest(id) else {
            return Ok(None);
        };
        let view = self.view_for(state, &manifest).await?;
        let nodes = db::list_nodes(&state.db).await?;
        let now = db::now_ts();
        let statuses: BTreeMap<String, ExtensionStatus> =
            store::list_statuses(&state.db, id)
                .await?
                .into_iter()
                .map(|row| (row.node_id.clone(), row.view()))
                .collect();
        let records: BTreeMap<String, store::ExtensionRecordRow> =
            store::list_records(&state.db, id)
                .await?
                .into_iter()
                .map(|row| (row.node_id.clone(), row))
                .collect();
        let cloudflared_settings = if id == CLOUDFLARED_ID {
            store::list_node_settings(&state.db).await?
        } else {
            Vec::new()
        };
        let requests = store::requests_for_extension(&state.db, id).await?;
        let mut node_views = Vec::with_capacity(nodes.len());
        for node in nodes {
            let node_id = node.id.clone();
            let record = records.get(&node_id);
            let settings = cloudflared_settings
                .iter()
                .find(|s| s.node_id == node_id);
            let active = self
                .is_active(state, &manifest, &node)
                .await
                .unwrap_or(false);
            let activation_revision = store::get_activation(&state.db, id, node.uuid())
                .await?
                .map(|row| row.revision())
                .unwrap_or(0);
            let (enabled, revision, config, updated_at) = if id == CLOUDFLARED_ID {
                let settings = settings;
                (
                    settings.is_some(),
                    settings.map(|s| s.revision()).unwrap_or(0),
                    settings
                        .map(|s| {
                            serde_json::json!({
                                "binary": s.binary,
                                "tunnel_id": s.tunnel_id,
                                "has_credentials": s.has_credentials(),
                            })
                        })
                        .unwrap_or(serde_json::Value::Null),
                    settings.map(|s| db::ts_to_dt(s.updated_at)).unwrap_or_else(Utc::now),
                )
            } else {
                (
                    record.map(|r| r.enabled()).unwrap_or(false),
                    record.map(|r| r.revision()).unwrap_or(0),
                    record
                        .map(|r| r.config_value())
                        .unwrap_or(serde_json::Value::Null),
                    record.map(|r| r.updated_at()).unwrap_or_else(Utc::now),
                )
            };
            node_views.push(ExtensionNodeView {
                node_id: node.uuid(),
                node_name: node.name.clone(),
                overlay_ip: node.overlay_ip(),
                online: node.online(now) || state.relay.is_online(node.uuid()),
                enabled,
                active,
                activation_revision,
                revision,
                config,
                requested: requests.contains_key(&node_id),
                request_message: requests.get(&node_id).cloned(),
                status: statuses.get(&node_id).cloned(),
                updated_at: enabled.then_some(updated_at),
            });
        }
        Ok(Some(ExtensionDetail {
            extension: view,
            nodes: node_views,
        }))
    }
}

async fn generic_desired_for(
    state: &AppState,
    node: &NodeRow,
    manifest: &ExtensionManifest,
) -> anyhow::Result<Option<ExtensionDesired>> {
    let Some(record) = store::get_record(&state.db, &manifest.id, node.uuid()).await? else {
        return Ok(None);
    };
    if !record.enabled() {
        return Ok(None);
    }
    let config = record.config_value();
    if manifest.node_driver == NodeDriver::Exec {
        let cfg: wm_ext::ExecConfig = serde_json::from_value(config.clone())
            .map_err(|err| anyhow::anyhow!("config exec inválida: {err}"))?;
        cfg.validate().map_err(anyhow::Error::msg)?;
    }
    Ok(Some(ExtensionDesired {
        id: manifest.id.clone(),
        name: manifest.name.clone(),
        driver: manifest.node_driver.as_str().to_string(),
        version: manifest.version.clone(),
        revision: record.revision(),
        activation_revision: 0,
        enabled: true,
        hooks: manifest.hooks.clone(),
        config,
    }))
}

// ---------------------------------------------------------------------------
// API views
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ExtensionView {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    pub builtin: bool,
    /// `public` (catálogo aberto) ou `private` (interno/operador).
    pub visibility: String,
    pub node_driver: String,
    pub ui: String,
    pub config_example: serde_json::Value,
    pub nodes_configured: usize,
    pub nodes_running: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ExtensionNodeView {
    pub node_id: Uuid,
    pub node_name: String,
    pub overlay_ip: std::net::Ipv4Addr,
    pub online: bool,
    pub enabled: bool,
    pub active: bool,
    pub activation_revision: u64,
    pub revision: u64,
    pub config: serde_json::Value,
    /// O servidor pediu que o nó rode esta extensão (recomendação).
    #[serde(default)]
    pub requested: bool,
    #[serde(default)]
    pub request_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<ExtensionStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct ExtensionDetail {
    #[serde(flatten)]
    pub extension: ExtensionView,
    pub nodes: Vec<ExtensionNodeView>,
}

#[derive(Debug, Serialize)]
pub struct ReloadResult {
    pub manifests: Vec<ExtensionView>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct ActivationInput {
    pub active: bool,
}

#[derive(Debug, Deserialize)]
pub struct RequestInput {
    #[serde(default)]
    pub requested: bool,
    #[serde(default)]
    pub message: String,
}

// ---------------------------------------------------------------------------
// Generic admin handlers
// ---------------------------------------------------------------------------

pub async fn list_extensions(
    State(state): State<Arc<AppState>>,
    _auth: AdminAuth,
) -> Result<Json<Vec<ExtensionView>>, ApiErr> {
    Ok(Json(state.extensions.views(&state).await?))
}

pub async fn get_extension(
    State(state): State<Arc<AppState>>,
    _auth: AdminAuth,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<ExtensionDetail>, ApiErr> {
    state
        .extensions
        .detail(&state, &id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiErr::not_found("extension_not_found", "extensão não encontrada"))
}

pub async fn reload_extensions(
    State(state): State<Arc<AppState>>,
    auth: AdminAuth,
    headers: axum::http::HeaderMap,
) -> Result<Json<ReloadResult>, ApiErr> {
    auth.require_csrf(&headers)?;
    let warnings = state.extensions.reload()?;
    // Apply new adapter effects immediately; exposes are already recomputed on
    // the next map build, shares/labels otherwise wait for the 30s loop.
    if let Err(err) = adapters::sync_all(&state).await {
        tracing::warn!(%err, "adapter sync após reload falhou");
    }
    // A manifest added/removed only reaches agents when the long-poll wakes up.
    state.bump_version();
    db::audit(
        &state.db,
        &format!("admin:{}", auth.username()),
        "extension.reload",
        "extensions",
        Some(&format!("warnings={}", warnings.len())),
        client_ip(&headers).as_deref(),
    )
    .await
    .ok();
    Ok(Json(ReloadResult {
        manifests: state.extensions.views(&state).await?,
        warnings,
    }))
}

#[derive(Debug, Deserialize)]
pub struct NodeConfigInput {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub config: serde_json::Value,
}

fn default_true() -> bool {
    true
}

pub async fn get_extension_node(
    State(state): State<Arc<AppState>>,
    _auth: AdminAuth,
    AxumPath((id, node_id)): AxumPath<(String, Uuid)>,
) -> Result<Json<ExtensionNodeView>, ApiErr> {
    let detail = state
        .extensions
        .detail(&state, &id)
        .await?
        .ok_or_else(|| ApiErr::not_found("extension_not_found", "extensão não encontrada"))?;
    detail
        .nodes
        .into_iter()
        .find(|n| n.node_id == node_id)
        .map(Json)
        .ok_or_else(|| ApiErr::not_found("node_not_found", "nó não encontrado"))
}

pub async fn put_extension_node(
    State(state): State<Arc<AppState>>,
    auth: AdminAuth,
    headers: axum::http::HeaderMap,
    AxumPath((id, node_id)): AxumPath<(String, Uuid)>,
    Json(input): Json<NodeConfigInput>,
) -> Result<Json<ExtensionNodeView>, ApiErr> {
    auth.require_csrf(&headers)?;
    let manifest = state
        .extensions
        .manifest(&id)
        .ok_or_else(|| ApiErr::not_found("extension_not_found", "extensão não encontrada"))?;
    if state.extensions.is_builtin(&id) {
        return Err(ApiErr::bad_request(
            "extension_managed",
            "esta extensão tem API própria e não usa configuração genérica",
        ));
    }
    let Some(node) = db::node_by_id(&state.db, node_id).await? else {
        return Err(ApiErr::not_found("node_not_found", "nó não encontrado"));
    };
    validate_generic_config(&manifest, &input.config)?;
    store::upsert_record(
        &state.db,
        &id,
        node_id,
        input.enabled,
        &input.config,
        &format!("admin:{}", auth.username()),
    )
    .await?;
    db::audit(
        &state.db,
        &format!("admin:{}", auth.username()),
        "extension.config",
        &format!("{id}/{}", node.name),
        Some(&format!("enabled={}", input.enabled)),
        client_ip(&headers).as_deref(),
    )
    .await
    .ok();
    state.bump_version();
    let detail = state
        .extensions
        .detail(&state, &id)
        .await?
        .ok_or_else(|| ApiErr::not_found("extension_not_found", "extensão não encontrada"))?;
    detail
        .nodes
        .into_iter()
        .find(|n| n.node_id == node_id)
        .map(Json)
        .ok_or_else(|| ApiErr::not_found("node_not_found", "nó não encontrado"))
}

pub async fn delete_extension_node(
    State(state): State<Arc<AppState>>,
    auth: AdminAuth,
    headers: axum::http::HeaderMap,
    AxumPath((id, node_id)): AxumPath<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    auth.require_csrf(&headers)?;
    if state.extensions.is_builtin(&id) {
        return Err(ApiErr::bad_request(
            "extension_managed",
            "esta extensão tem API própria e não usa configuração genérica",
        ));
    }
    if state.extensions.manifest(&id).is_none() {
        return Err(ApiErr::not_found("extension_not_found", "extensão não encontrada"));
    }
    let removed = store::delete_record(&state.db, &id, node_id).await?;
    if !removed {
        return Err(ApiErr::not_found(
            "config_not_found",
            "não há configuração desta extensão para o nó",
        ));
    }
    db::audit(
        &state.db,
        &format!("admin:{}", auth.username()),
        "extension.unconfig",
        &format!("{id}/{node_id}"),
        None,
        client_ip(&headers).as_deref(),
    )
    .await
    .ok();
    state.bump_version();
    Ok(Json(serde_json::json!({ "ok": true })))
}

fn validate_generic_config(
    manifest: &ExtensionManifest,
    config: &serde_json::Value,
) -> Result<(), ApiErr> {
    match manifest.node_driver {
        NodeDriver::Exec => {
            let cfg: wm_ext::ExecConfig = serde_json::from_value(config.clone()).map_err(|err| {
                ApiErr::bad_request(
                    "invalid_config",
                    format!("configuração do driver exec inválida: {err}"),
                )
            })?;
            cfg.validate()
                .map_err(|err| ApiErr::bad_request("invalid_config", err))
        }
        NodeDriver::None => Ok(()),
        NodeDriver::Cloudflared => Err(ApiErr::bad_request(
            "invalid_driver",
            "o driver cloudflared é reservado à extensão embutida",
        )),
    }
}

pub async fn put_request(
    State(state): State<Arc<AppState>>,
    auth: AdminAuth,
    headers: axum::http::HeaderMap,
    AxumPath((id, node_id)): AxumPath<(String, Uuid)>,
    Json(input): Json<RequestInput>,
) -> Result<Json<ExtensionNodeView>, ApiErr> {
    auth.require_csrf(&headers)?;
    let Some(node) = db::node_by_id(&state.db, node_id).await? else {
        return Err(ApiErr::not_found("node_not_found", "nó não encontrado"));
    };
    if state.extensions.manifest(&id).is_none() {
        return Err(ApiErr::not_found(
            "extension_not_found",
            "extensão não encontrada",
        ));
    }
    let message = input.message.trim();
    if message.chars().count() > 500 {
        return Err(ApiErr::bad_request(
            "request_too_long",
            "a mensagem da solicitação deve ter no máximo 500 caracteres",
        ));
    }
    let actor = format!("admin:{}", auth.username());
    store::set_request(
        &state.db,
        &id,
        node.uuid(),
        input.requested,
        message,
        &actor,
    )
    .await?;
    db::audit(
        &state.db,
        &actor,
        if input.requested {
            "extension.request"
        } else {
            "extension.unrequest"
        },
        &format!("{id}/{}", node.name),
        Some(message),
        client_ip(&headers).as_deref(),
    )
    .await
    .ok();
    let view = state
        .extensions
        .detail(&state, &id)
        .await?
        .and_then(|detail| {
            detail
                .nodes
                .into_iter()
                .find(|view| view.node_id == node.uuid())
        })
        .ok_or_else(|| ApiErr::internal("nó desapareceu do detalhe da extensão"))?;
    Ok(Json(view))
}

pub async fn put_activation(
    State(state): State<Arc<AppState>>,
    auth: AdminAuth,
    headers: axum::http::HeaderMap,
    AxumPath((id, node_id)): AxumPath<(String, Uuid)>,
    Json(input): Json<ActivationInput>,
) -> Result<Json<ExtensionNodeView>, ApiErr> {
    auth.require_csrf(&headers)?;
    let Some(node) = db::node_by_id(&state.db, node_id).await? else {
        return Err(ApiErr::not_found("node_not_found", "nó não encontrado"));
    };
    let actor = format!("admin:{}", auth.username());
    if state
        .extensions
        .set_node_activation(&state, &node, &id, input.active, &actor)
        .await?
        .is_none()
    {
        return Err(ApiErr::not_found(
            "extension_not_found",
            "extensão não encontrada",
        ));
    }
    let view = state
        .extensions
        .detail(&state, &id)
        .await?
        .and_then(|detail| {
            detail
                .nodes
                .into_iter()
                .find(|view| view.node_id == node.uuid())
        })
        .ok_or_else(|| ApiErr::internal("nó desapareceu do detalhe da extensão"))?;
    db::audit(
        &state.db,
        &actor,
        if input.active {
            "extension.activate"
        } else {
            "extension.deactivate"
        },
        &format!("{id}/{}", node.name),
        None,
        client_ip(&headers).as_deref(),
    )
    .await
    .ok();
    state.bump_version();
    Ok(Json(view))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router(state: Arc<AppState>) -> Router<Arc<AppState>> {
    let mut router = Router::new()
        .route("/admin/extensions", get(list_extensions))
        .route("/admin/extensions/reload", post(reload_extensions))
        .route("/admin/extensions/{id}", get(get_extension))
        .route(
            "/admin/extensions/{id}/nodes/{node_id}/config",
            get(get_extension_node)
                .put(put_extension_node)
                .delete(delete_extension_node),
        )
        .route(
            "/admin/extensions/{id}/nodes/{node_id}/activation",
            put(put_activation),
        )
        .route(
            "/admin/extensions/{id}/nodes/{node_id}/request",
            put(put_request),
        )
        .route("/node/extensions", get(node::list))
        .route("/node/extensions/{id}", put(node::set_active));
    for extension in state.extensions.code_extensions() {
        router = router.merge(extension.router());
    }
    router
}
