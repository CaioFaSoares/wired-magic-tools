//! Persistence for the extension host and the built-in cloudflared extension.

use std::collections::BTreeMap;

use anyhow::anyhow;
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

macro_rules! impl_from_row {
    ($ty:ty { $($field:ident),+ $(,)? }) => {
        impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for $ty {
            fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
                use sqlx::Row;
                Ok(Self {
                    $($field: row.try_get(stringify!($field))?),+
                })
            }
        }
    };
}
use uuid::Uuid;

use wm_proto::{ExtensionState, ExtensionStatus, NodeId};

use crate::db::{now_ts, opt_dt, ts_to_dt};

// ---------------------------------------------------------------------------
// Generic extension records and status
// ---------------------------------------------------------------------------

pub struct ExtensionRecordRow {
    pub extension_id: String,
    pub node_id: String,
    pub enabled: i64,
    pub config: String,
    pub revision: i64,
    pub updated_at: i64,
    pub updated_by: Option<String>,
}

impl_from_row!(ExtensionRecordRow {
    extension_id,
    node_id,
    enabled,
    config,
    revision,
    updated_at,
    updated_by
});

impl ExtensionRecordRow {
    pub fn node_uuid(&self) -> Uuid {
        Uuid::parse_str(&self.node_id).unwrap_or_else(|_| Uuid::new_v4())
    }

    pub fn enabled(&self) -> bool {
        self.enabled != 0
    }

    pub fn revision(&self) -> u64 {
        self.revision.max(0) as u64
    }

    pub fn config_value(&self) -> serde_json::Value {
        serde_json::from_str(&self.config).unwrap_or(serde_json::Value::Null)
    }

    pub fn updated_at(&self) -> DateTime<Utc> {
        ts_to_dt(self.updated_at)
    }
}

pub async fn get_record(
    pool: &SqlitePool,
    extension_id: &str,
    node_id: Uuid,
) -> anyhow::Result<Option<ExtensionRecordRow>> {
    Ok(sqlx::query_as(
        "SELECT * FROM extension_records WHERE extension_id = ? AND node_id = ?",
    )
    .bind(extension_id)
    .bind(node_id.to_string())
    .fetch_optional(pool)
    .await?)
}

pub async fn list_records(pool: &SqlitePool, extension_id: &str) -> anyhow::Result<Vec<ExtensionRecordRow>> {
    Ok(sqlx::query_as(
        "SELECT * FROM extension_records WHERE extension_id = ? ORDER BY node_id",
    )
    .bind(extension_id)
    .fetch_all(pool)
    .await?)
}

pub async fn list_records_for_node(
    pool: &SqlitePool,
    node_id: Uuid,
) -> anyhow::Result<Vec<ExtensionRecordRow>> {
    Ok(sqlx::query_as("SELECT * FROM extension_records WHERE node_id = ?")
        .bind(node_id.to_string())
        .fetch_all(pool)
        .await?)
}

/// Inserts or replaces one node's config and bumps its revision. The revision
/// is what lets an explicit restart reach an agent even with identical JSON.
pub async fn upsert_record(
    pool: &SqlitePool,
    extension_id: &str,
    node_id: Uuid,
    enabled: bool,
    config: &serde_json::Value,
    updated_by: &str,
) -> anyhow::Result<ExtensionRecordRow> {
    let now = now_ts();
    let encoded = serde_json::to_string(config)?;
    sqlx::query(
        "INSERT INTO extension_records(extension_id, node_id, enabled, config, revision, updated_at, updated_by)
         VALUES(?,?,?,?,1,?,?)
         ON CONFLICT(extension_id, node_id) DO UPDATE SET
           enabled = excluded.enabled,
           config = excluded.config,
           revision = extension_records.revision + 1,
           updated_at = excluded.updated_at,
           updated_by = excluded.updated_by",
    )
    .bind(extension_id)
    .bind(node_id.to_string())
    .bind(i64::from(enabled))
    .bind(encoded)
    .bind(now)
    .bind(updated_by)
    .execute(pool)
    .await?;
    get_record(pool, extension_id, node_id)
        .await?
        .ok_or_else(|| anyhow!("registro de extensão desapareceu após gravação"))
}

pub async fn delete_record(
    pool: &SqlitePool,
    extension_id: &str,
    node_id: Uuid,
) -> anyhow::Result<bool> {
    let result = sqlx::query("DELETE FROM extension_records WHERE extension_id = ? AND node_id = ?")
        .bind(extension_id)
        .bind(node_id.to_string())
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub struct ExtensionStatusRow {
    pub extension_id: String,
    pub node_id: String,
    pub name: String,
    pub driver: String,
    pub state: String,
    pub message: Option<String>,
    pub public_urls: String,
    pub details: String,
    pub updated_at: i64,
}

impl_from_row!(ExtensionStatusRow {
    extension_id,
    node_id,
    name,
    driver,
    state,
    message,
    public_urls,
    details,
    updated_at
});

impl ExtensionStatusRow {
    pub fn view(&self) -> ExtensionStatus {
        ExtensionStatus {
            id: self.extension_id.clone(),
            name: self.name.clone(),
            driver: self.driver.clone(),
            state: match self.state.as_str() {
                "running" => ExtensionState::Running,
                "starting" => ExtensionState::Starting,
                "error" => ExtensionState::Error,
                _ => ExtensionState::Stopped,
            },
            message: self.message.clone(),
            public_urls: serde_json::from_str(&self.public_urls).unwrap_or_default(),
            details: serde_json::from_str(&self.details)
                .unwrap_or(serde_json::Value::Null),
            updated_at: ts_to_dt(self.updated_at),
        }
    }

    pub fn online(&self) -> bool {
        self.state == "running"
    }
}

pub async fn upsert_statuses(
    pool: &SqlitePool,
    node_id: NodeId,
    statuses: &[ExtensionStatus],
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    for status in statuses {
        sqlx::query(
            "INSERT INTO extension_status(extension_id, node_id, name, driver, state, message, public_urls, details, updated_at)
             VALUES(?,?,?,?,?,?,?,?,?)
             ON CONFLICT(extension_id, node_id) DO UPDATE SET
               name = excluded.name,
               driver = excluded.driver,
               state = excluded.state,
               message = excluded.message,
               public_urls = excluded.public_urls,
               details = excluded.details,
               updated_at = excluded.updated_at",
        )
        .bind(&status.id)
        .bind(node_id.to_string())
        .bind(&status.name)
        .bind(&status.driver)
        .bind(status.state.to_string())
        .bind(&status.message)
        .bind(serde_json::to_string(&status.public_urls)?)
        .bind(serde_json::to_string(&status.details)?)
        .bind(now_ts())
        .execute(&mut *tx)
        .await?;
    }
    let reported: std::collections::HashSet<&str> =
        statuses.iter().map(|status| status.id.as_str()).collect();
    let existing: Vec<(String,)> =
        sqlx::query_as("SELECT extension_id FROM extension_status WHERE node_id = ?")
            .bind(node_id.to_string())
            .fetch_all(&mut *tx)
            .await?;
    for (extension_id,) in existing {
        if !reported.contains(extension_id.as_str()) {
            sqlx::query(
                "DELETE FROM extension_status WHERE node_id = ? AND extension_id = ?",
            )
            .bind(node_id.to_string())
            .bind(&extension_id)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

pub async fn get_status(
    pool: &SqlitePool,
    extension_id: &str,
    node_id: NodeId,
) -> anyhow::Result<Option<ExtensionStatusRow>> {
    Ok(sqlx::query_as(
        "SELECT * FROM extension_status WHERE extension_id = ? AND node_id = ?",
    )
    .bind(extension_id)
    .bind(node_id.to_string())
    .fetch_optional(pool)
    .await?)
}

pub async fn delete_status(
    pool: &SqlitePool,
    extension_id: &str,
    node_id: NodeId,
) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM extension_status WHERE extension_id = ? AND node_id = ?")
        .bind(extension_id)
        .bind(node_id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn list_statuses(
    pool: &SqlitePool,
    extension_id: &str,
) -> anyhow::Result<Vec<ExtensionStatusRow>> {
    Ok(sqlx::query_as(
        "SELECT * FROM extension_status WHERE extension_id = ? ORDER BY node_id",
    )
    .bind(extension_id)
    .fetch_all(pool)
    .await?)
}

pub async fn list_statuses_for_node(
    pool: &SqlitePool,
    node_id: NodeId,
) -> anyhow::Result<Vec<ExtensionStatusRow>> {
    Ok(sqlx::query_as("SELECT * FROM extension_status WHERE node_id = ? ORDER BY extension_id")
        .bind(node_id.to_string())
        .fetch_all(pool)
        .await?)
}

// ---------------------------------------------------------------------------
// cloudflared routes and node settings
// ---------------------------------------------------------------------------

pub struct CloudflaredRouteRow {
    pub id: String,
    pub project: String,
    pub node_id: String,
    pub local_url: String,
    pub mode: String,
    pub hostname: Option<String>,
    pub enabled: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

impl_from_row!(CloudflaredRouteRow {
    id,
    project,
    node_id,
    local_url,
    mode,
    hostname,
    enabled,
    created_at,
    updated_at
});

impl CloudflaredRouteRow {
    pub fn uuid(&self) -> Uuid {
        Uuid::parse_str(&self.id).unwrap_or_else(|_| Uuid::new_v4())
    }

    pub fn node_uuid(&self) -> Uuid {
        Uuid::parse_str(&self.node_id).unwrap_or_else(|_| Uuid::new_v4())
    }

    pub fn enabled(&self) -> bool {
        self.enabled != 0
    }

    pub fn mode(&self) -> wm_ext::CloudflaredMode {
        wm_ext::CloudflaredMode::parse(&self.mode).unwrap_or(wm_ext::CloudflaredMode::Quick)
    }

    pub fn created_at(&self) -> DateTime<Utc> {
        ts_to_dt(self.created_at)
    }

    pub fn updated_at(&self) -> DateTime<Utc> {
        ts_to_dt(self.updated_at)
    }
}

pub async fn list_routes(pool: &SqlitePool) -> anyhow::Result<Vec<CloudflaredRouteRow>> {
    Ok(
        sqlx::query_as("SELECT * FROM cloudflared_routes ORDER BY created_at, project")
            .fetch_all(pool)
            .await?,
    )
}

pub async fn list_routes_for_node(
    pool: &SqlitePool,
    node_id: Uuid,
) -> anyhow::Result<Vec<CloudflaredRouteRow>> {
    Ok(sqlx::query_as(
        "SELECT * FROM cloudflared_routes WHERE node_id = ? ORDER BY project",
    )
    .bind(node_id.to_string())
    .fetch_all(pool)
    .await?)
}

pub async fn get_route(pool: &SqlitePool, id: Uuid) -> anyhow::Result<Option<CloudflaredRouteRow>> {
    Ok(sqlx::query_as("SELECT * FROM cloudflared_routes WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(pool)
        .await?)
}

pub struct NewCloudflaredRoute<'a> {
    pub project: &'a str,
    pub node_id: Uuid,
    pub local_url: &'a str,
    pub mode: wm_ext::CloudflaredMode,
    pub hostname: Option<&'a str>,
    pub enabled: bool,
}

pub async fn create_route(
    pool: &SqlitePool,
    route: &NewCloudflaredRoute<'_>,
) -> anyhow::Result<CloudflaredRouteRow> {
    let id = Uuid::new_v4();
    let now = now_ts();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO cloudflared_routes(id, project, node_id, local_url, mode, hostname, enabled, created_at, updated_at)
         VALUES(?,?,?,?,?,?,?,?,?)",
    )
    .bind(id.to_string())
    .bind(route.project)
    .bind(route.node_id.to_string())
    .bind(route.local_url)
    .bind(route.mode.as_str())
    .bind(route.hostname)
    .bind(i64::from(route.enabled))
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    bump_revision_tx(&mut tx, route.node_id).await?;
    tx.commit().await?;
    get_route(pool, id)
        .await?
        .ok_or_else(|| anyhow!("rota desapareceu após gravação"))
}

pub async fn update_route(
    pool: &SqlitePool,
    id: Uuid,
    route: &NewCloudflaredRoute<'_>,
) -> anyhow::Result<CloudflaredRouteRow> {
    let current = get_route(pool, id)
        .await?
        .ok_or_else(|| anyhow!("rota não encontrada"))?;
    let now = now_ts();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE cloudflared_routes SET project = ?, node_id = ?, local_url = ?, mode = ?, hostname = ?, enabled = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(route.project)
    .bind(route.node_id.to_string())
    .bind(route.local_url)
    .bind(route.mode.as_str())
    .bind(route.hostname)
    .bind(i64::from(route.enabled))
    .bind(now)
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?;
    bump_revision_tx(&mut tx, current.node_uuid()).await?;
    if current.node_uuid() != route.node_id {
        bump_revision_tx(&mut tx, route.node_id).await?;
    }
    tx.commit().await?;
    get_route(pool, id)
        .await?
        .ok_or_else(|| anyhow!("rota desapareceu após atualização"))
}

pub async fn delete_route(pool: &SqlitePool, id: Uuid) -> anyhow::Result<Option<CloudflaredRouteRow>> {
    let current = get_route(pool, id).await?;
    let Some(current) = current else {
        return Ok(None);
    };
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM cloudflared_routes WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    bump_revision_tx(&mut tx, current.node_uuid()).await?;
    tx.commit().await?;
    Ok(Some(current))
}

pub struct CloudflaredNodeRow {
    pub node_id: String,
    pub binary: String,
    pub tunnel_id: Option<String>,
    pub credentials: Option<String>,
    pub revision: i64,
    pub updated_at: i64,
}

impl_from_row!(CloudflaredNodeRow {
    node_id,
    binary,
    tunnel_id,
    credentials,
    revision,
    updated_at
});

impl CloudflaredNodeRow {
    pub fn node_uuid(&self) -> Uuid {
        Uuid::parse_str(&self.node_id).unwrap_or_else(|_| Uuid::new_v4())
    }

    pub fn revision(&self) -> u64 {
        self.revision.max(0) as u64
    }

    pub fn has_credentials(&self) -> bool {
        self.credentials
            .as_deref()
            .map(|c| !c.trim().is_empty())
            .unwrap_or(false)
    }
}

pub async fn get_node_settings(
    pool: &SqlitePool,
    node_id: Uuid,
) -> anyhow::Result<Option<CloudflaredNodeRow>> {
    Ok(sqlx::query_as("SELECT * FROM cloudflared_nodes WHERE node_id = ?")
        .bind(node_id.to_string())
        .fetch_optional(pool)
        .await?)
}

pub async fn list_node_settings(pool: &SqlitePool) -> anyhow::Result<Vec<CloudflaredNodeRow>> {
    Ok(sqlx::query_as("SELECT * FROM cloudflared_nodes ORDER BY node_id")
        .fetch_all(pool)
        .await?)
}

pub struct CloudflaredNodeSettings<'a> {
    pub binary: &'a str,
    pub tunnel_id: Option<&'a str>,
    pub credentials: Option<&'a str>,
}

pub async fn upsert_node_settings(
    pool: &SqlitePool,
    node_id: Uuid,
    settings: &CloudflaredNodeSettings<'_>,
) -> anyhow::Result<CloudflaredNodeRow> {
    let now = now_ts();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO cloudflared_nodes(node_id, binary, tunnel_id, credentials, revision, updated_at)
         VALUES(?,?,?,?,1,?)
         ON CONFLICT(node_id) DO UPDATE SET
           binary = excluded.binary,
           tunnel_id = excluded.tunnel_id,
           credentials = excluded.credentials,
           revision = cloudflared_nodes.revision + 1,
           updated_at = excluded.updated_at",
    )
    .bind(node_id.to_string())
    .bind(settings.binary)
    .bind(settings.tunnel_id)
    .bind(settings.credentials)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    get_node_settings(pool, node_id)
        .await?
        .ok_or_else(|| anyhow!("configuração do cloudflared desapareceu após gravação"))
}

/// Increments the per-node revision without changing settings. Agents treat a
/// revision change as a request to restart the extension.
pub async fn bump_node_revision(pool: &SqlitePool, node_id: Uuid) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    bump_revision_tx(&mut tx, node_id).await?;
    tx.commit().await?;
    Ok(())
}

async fn bump_revision_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    node_id: Uuid,
) -> anyhow::Result<()> {
    let now = now_ts();
    sqlx::query(
        "INSERT INTO cloudflared_nodes(node_id, binary, revision, updated_at)
         VALUES(?, 'cloudflared', 1, ?)
         ON CONFLICT(node_id) DO UPDATE SET
           revision = cloudflared_nodes.revision + 1,
           updated_at = excluded.updated_at",
    )
    .bind(node_id.to_string())
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers shared by views
// ---------------------------------------------------------------------------

pub async fn status_map_for_node(
    pool: &SqlitePool,
    node_id: NodeId,
) -> anyhow::Result<BTreeMap<String, ExtensionStatusRow>> {
    Ok(list_statuses_for_node(pool, node_id)
        .await?
        .into_iter()
        .map(|row| (row.extension_id.clone(), row))
        .collect())
}

pub fn opt_updated_at(ts: Option<i64>) -> Option<DateTime<Utc>> {
    opt_dt(ts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    async fn pool() -> (tempfile::TempDir, SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::open(&dir.path().join("t.db")).await.unwrap();
        (dir, pool)
    }

    #[tokio::test]
    async fn records_bump_revision_and_delete() {
        let (_dir, pool) = pool().await;
        let node = Uuid::new_v4();
        let value = serde_json::json!({"command": "x"});
        let first = upsert_record(&pool, "hello", node, true, &value, "test")
            .await
            .unwrap();
        assert_eq!(first.revision(), 1);
        let second = upsert_record(&pool, "hello", node, false, &value, "test")
            .await
            .unwrap();
        assert_eq!(second.revision(), 2);
        assert!(!second.enabled());
        assert_eq!(list_records_for_node(&pool, node).await.unwrap().len(), 1);
        assert!(delete_record(&pool, "hello", node).await.unwrap());
        assert!(list_records(&pool, "hello").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn status_roundtrip() {
        let (_dir, pool) = pool().await;
        let node = Uuid::new_v4();
        let status = ExtensionStatus {
            id: "hello".into(),
            name: "Hello".into(),
            driver: "exec".into(),
            state: ExtensionState::Running,
            message: Some("ok".into()),
            public_urls: vec!["https://example.com".into()],
            details: serde_json::json!({"pid": 42}),
            updated_at: Utc::now(),
        };
        upsert_statuses(&pool, node, &[status]).await.unwrap();
        let row = get_status(&pool, "hello", node).await.unwrap().unwrap();
        let view = row.view();
        assert_eq!(view.state, ExtensionState::Running);
        assert_eq!(view.public_urls, vec!["https://example.com"]);
        assert_eq!(view.details["pid"], 42);

        // A report with an empty list is authoritative: stale statuses go away.
        upsert_statuses(&pool, node, &[]).await.unwrap();
        assert!(get_status(&pool, "hello", node).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn route_changes_bump_node_revision() {
        let (_dir, pool) = pool().await;
        let node = Uuid::new_v4();
        let settings = CloudflaredNodeSettings {
            binary: "cloudflared",
            tunnel_id: None,
            credentials: None,
        };
        let before = upsert_node_settings(&pool, node, &settings).await.unwrap();
        let route = NewCloudflaredRoute {
            project: "site",
            node_id: node,
            local_url: "http://127.0.0.1:3000",
            mode: wm_ext::CloudflaredMode::Quick,
            hostname: None,
            enabled: true,
        };
        let created = create_route(&pool, &route).await.unwrap();
        let after = get_node_settings(&pool, node).await.unwrap().unwrap();
        assert!(after.revision() > before.revision());
        update_route(
            &pool,
            created.uuid(),
            &NewCloudflaredRoute {
                enabled: false,
                ..route
            },
        )
        .await
        .unwrap();
        assert!(!get_route(&pool, created.uuid()).await.unwrap().unwrap().enabled());
        delete_route(&pool, created.uuid()).await.unwrap();
        assert!(get_route(&pool, created.uuid()).await.unwrap().is_none());
    }
}

// ---------------------------------------------------------------------------
// Per-node activation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ActivationRow {
    pub extension_id: String,
    pub node_id: String,
    pub enabled: i64,
    pub revision: i64,
    pub updated_at: i64,
    pub updated_by: Option<String>,
}

impl_from_row!(ActivationRow {
    extension_id,
    node_id,
    enabled,
    revision,
    updated_at,
    updated_by,
});

impl ActivationRow {
    pub fn enabled(&self) -> bool {
        self.enabled != 0
    }

    pub fn revision(&self) -> u64 {
        self.revision.max(0) as u64
    }
}

pub async fn get_activation(
    pool: &SqlitePool,
    extension_id: &str,
    node_id: Uuid,
) -> anyhow::Result<Option<ActivationRow>> {
    Ok(sqlx::query_as(
        "SELECT * FROM extension_activations WHERE extension_id = ? AND node_id = ?",
    )
    .bind(extension_id)
    .bind(node_id.to_string())
    .fetch_optional(pool)
    .await?)
}

pub async fn list_activations_for_node(
    pool: &SqlitePool,
    node_id: Uuid,
) -> anyhow::Result<Vec<ActivationRow>> {
    Ok(sqlx::query_as(
        "SELECT * FROM extension_activations WHERE node_id = ? ORDER BY extension_id",
    )
    .bind(node_id.to_string())
    .fetch_all(pool)
    .await?)
}

/// Upserts the activation and bumps its revision. The revision is what tells
/// the agent to run activate/deactivate hooks; config edits do not touch it.
pub async fn set_activation(
    pool: &SqlitePool,
    extension_id: &str,
    node_id: Uuid,
    enabled: bool,
    actor: &str,
) -> anyhow::Result<ActivationRow> {
    sqlx::query(
        "INSERT INTO extension_activations(extension_id, node_id, enabled, revision, updated_at, updated_by)
         VALUES(?,?,?,1,?,?)
         ON CONFLICT(extension_id, node_id) DO UPDATE SET
           enabled = excluded.enabled,
           revision = extension_activations.revision + 1,
           updated_at = excluded.updated_at,
           updated_by = excluded.updated_by",
    )
    .bind(extension_id)
    .bind(node_id.to_string())
    .bind(i64::from(enabled))
    .bind(now_ts())
    .bind(actor)
    .execute(pool)
    .await?;
    get_activation(pool, extension_id, node_id)
        .await?
        .ok_or_else(|| anyhow!("ativação de extensão desapareceu após gravação"))
}

// ---------------------------------------------------------------------------
// Adapter resources (shares materialized from manifests)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct AdapterResourceRow {
    pub extension_id: String,
    pub node_id: String,
    pub resource: String,
    pub resource_id: String,
    pub updated_at: i64,
}

impl_from_row!(AdapterResourceRow {
    extension_id,
    node_id,
    resource,
    resource_id,
    updated_at,
});

pub async fn get_adapter_resource(
    pool: &SqlitePool,
    extension_id: &str,
    node_id: Uuid,
    resource: &str,
) -> anyhow::Result<Option<AdapterResourceRow>> {
    Ok(sqlx::query_as(
        "SELECT * FROM extension_adapter_resources
         WHERE extension_id = ? AND node_id = ? AND resource = ?",
    )
    .bind(extension_id)
    .bind(node_id.to_string())
    .bind(resource)
    .fetch_optional(pool)
    .await?)
}

pub async fn list_all_adapter_resources(
    pool: &SqlitePool,
) -> anyhow::Result<Vec<AdapterResourceRow>> {
    Ok(sqlx::query_as("SELECT * FROM extension_adapter_resources ORDER BY extension_id, node_id, resource")
        .fetch_all(pool)
        .await?)
}

pub async fn list_adapter_resources(
    pool: &SqlitePool,
    extension_id: &str,
    node_id: Uuid,
) -> anyhow::Result<Vec<AdapterResourceRow>> {
    Ok(sqlx::query_as(
        "SELECT * FROM extension_adapter_resources
         WHERE extension_id = ? AND node_id = ? ORDER BY resource",
    )
    .bind(extension_id)
    .bind(node_id.to_string())
    .fetch_all(pool)
    .await?)
}

pub async fn set_adapter_resource(
    pool: &SqlitePool,
    extension_id: &str,
    node_id: Uuid,
    resource: &str,
    resource_id: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO extension_adapter_resources(extension_id, node_id, resource, resource_id, updated_at)
         VALUES(?,?,?,?,?)
         ON CONFLICT(extension_id, node_id, resource) DO UPDATE SET
           resource_id = excluded.resource_id,
           updated_at = excluded.updated_at",
    )
    .bind(extension_id)
    .bind(node_id.to_string())
    .bind(resource)
    .bind(resource_id)
    .bind(now_ts())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete_adapter_resource(
    pool: &SqlitePool,
    extension_id: &str,
    node_id: Uuid,
    resource: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "DELETE FROM extension_adapter_resources
         WHERE extension_id = ? AND node_id = ? AND resource = ?",
    )
    .bind(extension_id)
    .bind(node_id.to_string())
    .bind(resource)
    .execute(pool)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Server requests ("o servidor pediu para rodar esta extensão")
// ---------------------------------------------------------------------------

pub struct ExtensionRequestRow {
    pub extension_id: String,
    pub node_id: String,
    pub message: String,
    pub updated_at: i64,
    pub updated_by: Option<String>,
}

impl_from_row!(ExtensionRequestRow {
    extension_id,
    node_id,
    message,
    updated_at,
    updated_by
});

/// Creates or removes a request. An absent row means "não solicitada".
pub async fn set_request(
    pool: &SqlitePool,
    extension_id: &str,
    node_id: Uuid,
    requested: bool,
    message: &str,
    updated_by: &str,
) -> anyhow::Result<()> {
    if requested {
        sqlx::query(
            "INSERT INTO extension_requests(extension_id, node_id, message, updated_at, updated_by)
             VALUES(?,?,?,?,?)
             ON CONFLICT(extension_id, node_id) DO UPDATE SET
               message = excluded.message,
               updated_at = excluded.updated_at,
               updated_by = excluded.updated_by",
        )
        .bind(extension_id)
        .bind(node_id.to_string())
        .bind(message)
        .bind(now_ts())
        .bind(updated_by)
        .execute(pool)
        .await?;
    } else {
        sqlx::query("DELETE FROM extension_requests WHERE extension_id = ? AND node_id = ?")
            .bind(extension_id)
            .bind(node_id.to_string())
            .execute(pool)
            .await?;
    }
    Ok(())
}

pub async fn get_request(
    pool: &SqlitePool,
    extension_id: &str,
    node_id: Uuid,
) -> anyhow::Result<Option<ExtensionRequestRow>> {
    Ok(sqlx::query_as(
        "SELECT * FROM extension_requests WHERE extension_id = ? AND node_id = ?",
    )
    .bind(extension_id)
    .bind(node_id.to_string())
    .fetch_optional(pool)
    .await?)
}

/// Requests for one node, keyed by extension id.
pub async fn requests_for_node(
    pool: &SqlitePool,
    node_id: Uuid,
) -> anyhow::Result<BTreeMap<String, String>> {
    let rows: Vec<ExtensionRequestRow> =
        sqlx::query_as("SELECT * FROM extension_requests WHERE node_id = ?")
            .bind(node_id.to_string())
            .fetch_all(pool)
            .await?;
    Ok(rows
        .into_iter()
        .map(|row| (row.extension_id, row.message))
        .collect())
}

/// All requests for one extension, keyed by node id.
pub async fn requests_for_extension(
    pool: &SqlitePool,
    extension_id: &str,
) -> anyhow::Result<BTreeMap<String, String>> {
    let rows: Vec<ExtensionRequestRow> =
        sqlx::query_as("SELECT * FROM extension_requests WHERE extension_id = ?")
            .bind(extension_id)
            .fetch_all(pool)
            .await?;
    Ok(rows
        .into_iter()
        .map(|row| (row.node_id, row.message))
        .collect())
}
