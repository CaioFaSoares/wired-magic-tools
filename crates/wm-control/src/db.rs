use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use chrono::{DateTime, Utc};
use ipnet::Ipv4Net;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
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

use wm_proto::{
    AclPolicy, AclView, EndpointCandidate, JoinTokenView, NatType, NodeStatus, NodeView, PeerInfo,
    PeerStats,
};

pub const ONLINE_WINDOW_SECS: i64 = 90;

pub fn now_ts() -> i64 {
    Utc::now().timestamp()
}

pub fn ts_to_dt(ts: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(ts, 0).unwrap_or_else(Utc::now)
}

pub fn opt_dt(ts: Option<i64>) -> Option<DateTime<Utc>> {
    ts.map(ts_to_dt)
}

pub async fn open(path: &Path) -> anyhow::Result<SqlitePool> {
    let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
        .with_context(|| format!("abrindo {}", path.display()))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(opts)
        .await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    init_schema(&pool).await?;
    Ok(pool)
}

async fn init_schema(pool: &SqlitePool) -> anyhow::Result<()> {
    let stmts = [
        "CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        "CREATE TABLE IF NOT EXISTS admin_users (
            id TEXT PRIMARY KEY,
            username TEXT NOT NULL UNIQUE,
            password_hash TEXT NOT NULL,
            created_at INTEGER NOT NULL
        )",
        "CREATE TABLE IF NOT EXISTS admin_sessions (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            token_hash TEXT NOT NULL UNIQUE,
            csrf_token TEXT NOT NULL,
            expires_at INTEGER NOT NULL,
            created_at INTEGER NOT NULL,
            ip TEXT,
            ua TEXT
        )",
        "CREATE TABLE IF NOT EXISTS join_tokens (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            prefix TEXT NOT NULL,
            token_hash TEXT NOT NULL UNIQUE,
            tags TEXT NOT NULL DEFAULT '[]',
            labels TEXT NOT NULL DEFAULT '{}',
            max_uses INTEGER NOT NULL,
            uses INTEGER NOT NULL DEFAULT 0,
            expires_at INTEGER NOT NULL,
            created_at INTEGER NOT NULL,
            created_by TEXT,
            revoked_at INTEGER
        )",
        "CREATE TABLE IF NOT EXISTS nodes (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            hostname TEXT NOT NULL,
            os TEXT,
            arch TEXT,
            version TEXT,
            wg_public_key TEXT NOT NULL UNIQUE,
            overlay_ip TEXT NOT NULL UNIQUE,
            status TEXT NOT NULL DEFAULT 'pending',
            tags TEXT NOT NULL DEFAULT '[]',
            labels TEXT NOT NULL DEFAULT '{}',
            nat_type TEXT NOT NULL DEFAULT 'unknown',
            relay_connected INTEGER NOT NULL DEFAULT 0,
            endpoints TEXT NOT NULL DEFAULT '[]',
            reported_peers TEXT NOT NULL DEFAULT '[]',
            created_at INTEGER NOT NULL,
            approved_at INTEGER,
            disabled_at INTEGER,
            last_seen_at INTEGER,
            created_by TEXT
        )",
        "CREATE TABLE IF NOT EXISTS node_tokens (
            id TEXT PRIMARY KEY,
            node_id TEXT NOT NULL,
            token_hash TEXT NOT NULL UNIQUE,
            created_at INTEGER NOT NULL,
            last_used_at INTEGER,
            revoked_at INTEGER
        )",
        "CREATE TABLE IF NOT EXISTS acl_policies (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            version INTEGER NOT NULL,
            yaml TEXT NOT NULL,
            compiled TEXT NOT NULL,
            updated_by TEXT,
            updated_at INTEGER NOT NULL
        )",
        "CREATE TABLE IF NOT EXISTS service_tokens (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            token_hash TEXT NOT NULL UNIQUE,
            scopes TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            last_used_at INTEGER,
            revoked_at INTEGER
        )",
        "CREATE TABLE IF NOT EXISTS audit_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ts INTEGER NOT NULL,
            actor TEXT NOT NULL,
            action TEXT NOT NULL,
            target TEXT NOT NULL,
            detail TEXT,
            ip TEXT
        )",
        "CREATE TABLE IF NOT EXISTS peer_stats (
            node_id TEXT NOT NULL,
            peer_id TEXT NOT NULL,
            path TEXT,
            rtt_ms INTEGER,
            last_handshake_secs INTEGER,
            rx_bytes INTEGER NOT NULL DEFAULT 0,
            tx_bytes INTEGER NOT NULL DEFAULT 0,
            relay_online INTEGER NOT NULL DEFAULT 0,
            endpoint TEXT,
            last_error TEXT,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (node_id, peer_id)
        )",
        // Generic extension configuration, one row per (extension, node).
        // Extensions own the JSON shape; the host only stores and revisions it.
        "CREATE TABLE IF NOT EXISTS extension_records (
            extension_id TEXT NOT NULL,
            node_id TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1,
            config TEXT NOT NULL DEFAULT '{}',
            revision INTEGER NOT NULL DEFAULT 1,
            updated_at INTEGER NOT NULL,
            updated_by TEXT,
            PRIMARY KEY (extension_id, node_id)
        )",
        // Per-node activation state. The extension is only materialized on a
        // node after the operator/member activates it there.
        "CREATE TABLE IF NOT EXISTS extension_activations (
            extension_id TEXT NOT NULL,
            node_id TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 0,
            revision INTEGER NOT NULL DEFAULT 1,
            updated_at INTEGER NOT NULL,
            updated_by TEXT,
            PRIMARY KEY (extension_id, node_id)
        )",
        // Resources (shares) materialized by an extension adapter. Tracking is
        // explicit so deactivation never deletes a user-created share with the
        // same name.
        "CREATE TABLE IF NOT EXISTS extension_adapter_resources (
            extension_id TEXT NOT NULL,
            node_id TEXT NOT NULL,
            resource TEXT NOT NULL,
            resource_id TEXT NOT NULL,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (extension_id, node_id, resource)
        )",
        // Last status reported by each node for each extension.
        // A server-side request for a node to run an extension. It is a
        // recommendation, not an activation: the member still decides.
        "CREATE TABLE IF NOT EXISTS extension_requests (
            extension_id TEXT NOT NULL,
            node_id TEXT NOT NULL,
            message TEXT NOT NULL DEFAULT '',
            updated_at INTEGER NOT NULL,
            updated_by TEXT,
            PRIMARY KEY (extension_id, node_id)
        )",
        "CREATE TABLE IF NOT EXISTS extension_status (
            extension_id TEXT NOT NULL,
            node_id TEXT NOT NULL,
            name TEXT NOT NULL DEFAULT '',
            driver TEXT NOT NULL DEFAULT '',
            state TEXT NOT NULL,
            message TEXT,
            public_urls TEXT NOT NULL DEFAULT '[]',
            details TEXT NOT NULL DEFAULT 'null',
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (extension_id, node_id)
        )",
        // cloudflared routes and per-node tunnel settings. The extension owns
        // these tables; the generic extension host does not read them.
        "CREATE TABLE IF NOT EXISTS cloudflared_routes (
            id TEXT PRIMARY KEY,
            project TEXT NOT NULL,
            node_id TEXT NOT NULL,
            local_url TEXT NOT NULL,
            mode TEXT NOT NULL,
            hostname TEXT,
            enabled INTEGER NOT NULL DEFAULT 1,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        )",
        "CREATE TABLE IF NOT EXISTS cloudflared_nodes (
            node_id TEXT PRIMARY KEY,
            binary TEXT NOT NULL DEFAULT 'cloudflared',
            tunnel_id TEXT,
            credentials TEXT,
            revision INTEGER NOT NULL DEFAULT 1,
            updated_at INTEGER NOT NULL
        )",
        // Named services published by a node. The control plane owns the
        // registry; the map carries the resulting view to every agent.
        "CREATE TABLE IF NOT EXISTS shares (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            node_id TEXT NOT NULL,
            port INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
            kind TEXT NOT NULL DEFAULT 'web',
            protocol TEXT NOT NULL DEFAULT 'tcp',
            description TEXT NOT NULL DEFAULT '',
            enabled INTEGER NOT NULL DEFAULT 1,
            public_url TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS idx_nodes_last_seen ON nodes(last_seen_at)",
        "CREATE INDEX IF NOT EXISTS idx_join_tokens_hash ON join_tokens(token_hash)",
        "CREATE INDEX IF NOT EXISTS idx_audit_ts ON audit_events(ts DESC)",
        "CREATE INDEX IF NOT EXISTS idx_extension_status_extension ON extension_status(extension_id)",
        "CREATE INDEX IF NOT EXISTS idx_extension_requests_node ON extension_requests(node_id)",
        "CREATE INDEX IF NOT EXISTS idx_cloudflared_routes_node ON cloudflared_routes(node_id)",
        "CREATE INDEX IF NOT EXISTS idx_shares_node ON shares(node_id)",
    ];
    for stmt in stmts {
        sqlx::query(stmt).execute(pool).await?;
    }
    // The extension status table gained these columns during development.
    // Keep upgrades from a previous alpha data directory working.
    ensure_column(pool, "extension_status", "name", "name TEXT NOT NULL DEFAULT ''").await?;
    ensure_column(pool, "extension_status", "driver", "driver TEXT NOT NULL DEFAULT ''").await?;
    // Presentation identity is separate from the technical name used by ACL.
    ensure_column(pool, "nodes", "display_name", "display_name TEXT NOT NULL DEFAULT ''").await?;
    ensure_column(pool, "nodes", "about", "about TEXT NOT NULL DEFAULT ''").await?;
    // Operator requirements and the last evaluated compliance snapshot.
    ensure_column(pool, "nodes", "requirements", "requirements TEXT NOT NULL DEFAULT '{}'").await?;
    ensure_column(pool, "nodes", "compliance", "compliance TEXT NOT NULL DEFAULT '{}'").await?;
    ensure_column(pool, "nodes", "agent_runtime", "agent_runtime TEXT NOT NULL DEFAULT '{}'").await?;
    // Invitations can require explicit approval per enrollment; the default
    // stays "invitation alone is enough".
    ensure_column(pool, "join_tokens", "require_approval", "require_approval INTEGER NOT NULL DEFAULT 0").await?;
    Ok(())
}

async fn ensure_column(
    pool: &SqlitePool,
    table: &str,
    column: &str,
    definition: &str,
) -> anyhow::Result<()> {
    let pragma = format!("PRAGMA table_info({table})");
    let rows: Vec<(i64, String)> = sqlx::query_as(sqlx::AssertSqlSafe(pragma)).fetch_all(pool).await?;
    if rows.iter().any(|(_, name)| name == column) {
        return Ok(());
    }
    let alter = format!("ALTER TABLE {table} ADD COLUMN {definition}");
    sqlx::query(sqlx::AssertSqlSafe(alter)).execute(pool).await?;
    Ok(())
}

pub async fn get_setting(pool: &SqlitePool, key: &str) -> anyhow::Result<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as("SELECT value FROM settings WHERE key = ?")
        .bind(key)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.0))
}

pub async fn set_setting(pool: &SqlitePool, key: &str, value: &str) -> anyhow::Result<()> {
    sqlx::query("INSERT INTO settings(key, value) VALUES(?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value")
        .bind(key)
        .bind(value)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn ensure_admin(
    pool: &SqlitePool,
    username: &str,
    configured_password: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM admin_users")
        .fetch_one(pool)
        .await?;
    if count.0 > 0 {
        return Ok(None);
    }
    let (password, generated) = match configured_password {
        Some(p) if !p.is_empty() => (p.to_string(), false),
        _ => (crate::auth::random_password(18), true),
    };
    let hash = crate::auth::hash_password(&password)?;
    sqlx::query("INSERT INTO admin_users(id, username, password_hash, created_at) VALUES(?,?,?,?)")
        .bind(Uuid::new_v4().to_string())
        .bind(username)
        .bind(hash)
        .bind(now_ts())
        .execute(pool)
        .await?;
    Ok(generated.then_some(password))
}

pub struct AdminRow {
    pub id: String,
    pub username: String,
    pub password_hash: String,
    pub created_at: i64,
}

impl_from_row!(AdminRow { id, username, password_hash, created_at });

impl AdminRow {
    pub fn uuid(&self) -> Uuid {
        Uuid::parse_str(&self.id).unwrap_or_else(|_| Uuid::new_v4())
    }
}

pub async fn admin_by_username(
    pool: &SqlitePool,
    username: &str,
) -> anyhow::Result<Option<AdminRow>> {
    Ok(sqlx::query_as("SELECT * FROM admin_users WHERE username = ?")
        .bind(username)
        .fetch_optional(pool)
        .await?)
}

pub async fn admin_by_id(pool: &SqlitePool, id: &str) -> anyhow::Result<Option<AdminRow>> {
    Ok(sqlx::query_as("SELECT * FROM admin_users WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

pub struct SessionRow {
    pub id: String,
    pub user_id: String,
    pub token_hash: String,
    pub csrf_token: String,
    pub expires_at: i64,
    pub created_at: i64,
}

impl_from_row!(SessionRow {
    id,
    user_id,
    token_hash,
    csrf_token,
    expires_at,
    created_at
});

pub async fn create_admin_session(
    pool: &SqlitePool,
    user_id: &str,
    token_hash: &str,
    csrf: &str,
    ttl_secs: i64,
    ip: Option<&str>,
    ua: Option<&str>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO admin_sessions(id, user_id, token_hash, csrf_token, expires_at, created_at, ip, ua)
         VALUES(?,?,?,?,?,?,?,?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(user_id)
    .bind(token_hash)
    .bind(csrf)
    .bind(now_ts() + ttl_secs)
    .bind(now_ts())
    .bind(ip)
    .bind(ua)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn session_by_hash(
    pool: &SqlitePool,
    token_hash: &str,
) -> anyhow::Result<Option<SessionRow>> {
    Ok(sqlx::query_as(
        "SELECT id, user_id, token_hash, csrf_token, expires_at, created_at
         FROM admin_sessions WHERE token_hash = ? AND expires_at > ?",
    )
    .bind(token_hash)
    .bind(now_ts())
    .fetch_optional(pool)
    .await?)
}

pub async fn delete_session(pool: &SqlitePool, id: &str) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM admin_sessions WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub struct JoinTokenRow {
    pub id: String,
    pub name: String,
    pub prefix: String,
    pub token_hash: String,
    pub tags: String,
    pub labels: String,
    pub max_uses: i64,
    pub uses: i64,
    pub expires_at: i64,
    pub created_at: i64,
    pub created_by: Option<String>,
    pub revoked_at: Option<i64>,
    pub require_approval: i64,
}

impl_from_row!(JoinTokenRow {
    id,
    name,
    prefix,
    token_hash,
    tags,
    labels,
    max_uses,
    uses,
    expires_at,
    created_at,
    created_by,
    revoked_at,
    require_approval
});

impl JoinTokenRow {
    pub fn tags(&self) -> Vec<String> {
        serde_json::from_str(&self.tags).unwrap_or_default()
    }
    pub fn labels(&self) -> BTreeMap<String, String> {
        serde_json::from_str(&self.labels).unwrap_or_default()
    }
    pub fn view(&self) -> JoinTokenView {
        JoinTokenView {
            id: Uuid::parse_str(&self.id).unwrap_or_else(|_| Uuid::new_v4()),
            name: self.name.clone(),
            prefix: self.prefix.clone(),
            tags: self.tags(),
            labels: self.labels(),
            max_uses: self.max_uses,
            uses: self.uses,
            expires_at: ts_to_dt(self.expires_at),
            created_at: ts_to_dt(self.created_at),
            revoked_at: opt_dt(self.revoked_at),
            require_approval: self.require_approval != 0,
            token: None,
        }
    }
}

pub async fn create_join_token(
    pool: &SqlitePool,
    name: &str,
    prefix: &str,
    token_hash: &str,
    tags: &[String],
    labels: &BTreeMap<String, String>,
    max_uses: i64,
    ttl_secs: i64,
    created_by: &str,
) -> anyhow::Result<JoinTokenRow> {
    create_join_token_full(
        pool, name, prefix, token_hash, tags, labels, max_uses, ttl_secs, false, created_by,
    )
    .await
}

/// Creates an invitation that additionally requires explicit admin approval on
/// enrollment. Kept separate so existing callers keep the invitation-only
/// default.
#[allow(clippy::too_many_arguments)]
pub async fn create_join_token_with_approval(
    pool: &SqlitePool,
    name: &str,
    prefix: &str,
    token_hash: &str,
    tags: &[String],
    labels: &BTreeMap<String, String>,
    max_uses: i64,
    ttl_secs: i64,
    require_approval: bool,
    created_by: &str,
) -> anyhow::Result<JoinTokenRow> {
    create_join_token_full(
        pool, name, prefix, token_hash, tags, labels, max_uses, ttl_secs, require_approval,
        created_by,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn create_join_token_full(
    pool: &SqlitePool,
    name: &str,
    prefix: &str,
    token_hash: &str,
    tags: &[String],
    labels: &BTreeMap<String, String>,
    max_uses: i64,
    ttl_secs: i64,
    require_approval: bool,
    created_by: &str,
) -> anyhow::Result<JoinTokenRow> {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO join_tokens(id, name, prefix, token_hash, tags, labels, max_uses, uses, expires_at, created_at, created_by, require_approval)
         VALUES(?,?,?,?,?,?,?,0,?,?,?,?)",
    )
    .bind(id.to_string())
    .bind(name)
    .bind(prefix)
    .bind(token_hash)
    .bind(serde_json::to_string(tags)?)
    .bind(serde_json::to_string(labels)?)
    .bind(max_uses)
    .bind(now_ts() + ttl_secs)
    .bind(now_ts())
    .bind(created_by)
    .bind(if require_approval { 1i64 } else { 0i64 })
    .execute(pool)
    .await?;
    get_join_token(pool, id).await?.ok_or_else(|| anyhow!("join token sumiu"))
}

pub async fn get_join_token(pool: &SqlitePool, id: Uuid) -> anyhow::Result<Option<JoinTokenRow>> {
    Ok(sqlx::query_as("SELECT * FROM join_tokens WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(pool)
        .await?)
}

pub async fn list_join_tokens(pool: &SqlitePool) -> anyhow::Result<Vec<JoinTokenRow>> {
    Ok(sqlx::query_as("SELECT * FROM join_tokens ORDER BY created_at DESC")
        .fetch_all(pool)
        .await?)
}

pub async fn revoke_join_token(pool: &SqlitePool, id: Uuid) -> anyhow::Result<()> {
    sqlx::query("UPDATE join_tokens SET revoked_at = ? WHERE id = ?")
        .bind(now_ts())
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

pub struct NodeRow {
    pub id: String,
    pub name: String,
    pub hostname: String,
    pub os: Option<String>,
    pub arch: Option<String>,
    pub version: Option<String>,
    pub wg_public_key: String,
    pub overlay_ip: String,
    pub status: String,
    pub tags: String,
    pub labels: String,
    pub nat_type: String,
    pub relay_connected: i64,
    pub endpoints: String,
    pub reported_peers: String,
    pub created_at: i64,
    pub approved_at: Option<i64>,
    pub disabled_at: Option<i64>,
    pub last_seen_at: Option<i64>,
    pub created_by: Option<String>,
    pub display_name: String,
    pub about: String,
    pub requirements: String,
    pub compliance: String,
    pub agent_runtime: String,
}

impl_from_row!(NodeRow {
    id,
    name,
    hostname,
    os,
    arch,
    version,
    wg_public_key,
    overlay_ip,
    status,
    tags,
    labels,
    nat_type,
    relay_connected,
    endpoints,
    reported_peers,
    created_at,
    approved_at,
    disabled_at,
    last_seen_at,
    created_by,
    display_name,
    about,
    requirements,
    compliance,
    agent_runtime
});

impl NodeRow {
    pub fn uuid(&self) -> Uuid {
        Uuid::parse_str(&self.id).unwrap_or_else(|_| Uuid::new_v4())
    }
    pub fn overlay_ip(&self) -> Ipv4Addr {
        self.overlay_ip.parse().unwrap_or(Ipv4Addr::UNSPECIFIED)
    }
    pub fn status(&self) -> NodeStatus {
        match self.status.as_str() {
            "active" => NodeStatus::Active,
            "disabled" => NodeStatus::Disabled,
            _ => NodeStatus::Pending,
        }
    }
    pub fn tags(&self) -> Vec<String> {
        serde_json::from_str(&self.tags).unwrap_or_default()
    }
    pub fn labels(&self) -> BTreeMap<String, String> {
        serde_json::from_str(&self.labels).unwrap_or_default()
    }
    pub fn endpoints(&self) -> Vec<EndpointCandidate> {
        serde_json::from_str(&self.endpoints).unwrap_or_default()
    }
    pub fn nat_type(&self) -> NatType {
        match self.nat_type.as_str() {
            "cone" => NatType::Cone,
            "symmetric" => NatType::Symmetric,
            _ => NatType::Unknown,
        }
    }
    pub fn online(&self, now: i64) -> bool {
        self.last_seen_at
            .map(|ts| now.saturating_sub(ts) <= ONLINE_WINDOW_SECS)
            .unwrap_or(false)
    }
    /// Friendly name for panels; empty when the member did not configure one.
    pub fn display_name(&self) -> &str {
        &self.display_name
    }
    pub fn about(&self) -> &str {
        &self.about
    }
    pub fn requirements(&self) -> wm_proto::NodeRequirements {
        serde_json::from_str(&self.requirements).unwrap_or_default()
    }
    pub fn compliance(&self) -> wm_proto::NodeCompliance {
        serde_json::from_str(&self.compliance).unwrap_or_default()
    }
    pub fn agent_runtime(&self) -> wm_proto::AgentRuntime {
        serde_json::from_str(&self.agent_runtime).unwrap_or_default()
    }
    pub fn peer_info(&self, now: i64) -> PeerInfo {
        PeerInfo {
            id: self.uuid(),
            name: self.name.clone(),
            hostname: self.hostname.clone(),
            overlay_ip: self.overlay_ip(),
            wg_public_key: self.wg_public_key.clone(),
            tags: self.tags(),
            labels: self.labels(),
            endpoints: self.endpoints(),
            online: self.online(now),
            last_seen: opt_dt(self.last_seen_at),
            display_name: self.display_name.clone(),
            about: self.about.clone(),
        }
    }
    pub fn reported_peers(&self) -> Vec<PeerStats> {
        serde_json::from_str(&self.reported_peers).unwrap_or_default()
    }
    pub fn view(&self, now: i64) -> NodeView {
        NodeView {
            node: self.peer_info(now),
            status: self.status(),
            created_at: ts_to_dt(self.created_at),
            approved_at: opt_dt(self.approved_at),
            disabled_at: opt_dt(self.disabled_at),
            nat_type: self.nat_type(),
            relay_connected: self.relay_connected != 0,
            reported_peers: self.reported_peers(),
            requirements: self.requirements(),
            compliance: self.compliance(),
        }
    }
}

pub async fn list_nodes(pool: &SqlitePool) -> anyhow::Result<Vec<NodeRow>> {
    Ok(
        sqlx::query_as("SELECT * FROM nodes ORDER BY overlay_ip, name")
            .fetch_all(pool)
            .await?,
    )
}

pub async fn node_by_id(pool: &SqlitePool, id: Uuid) -> anyhow::Result<Option<NodeRow>> {
    Ok(sqlx::query_as("SELECT * FROM nodes WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(pool)
        .await?)
}

pub async fn node_by_name(pool: &SqlitePool, name: &str) -> anyhow::Result<Option<NodeRow>> {
    Ok(sqlx::query_as("SELECT * FROM nodes WHERE name = ?")
        .bind(name)
        .fetch_optional(pool)
        .await?)
}

pub async fn node_by_token_hash(
    pool: &SqlitePool,
    hash: &str,
) -> anyhow::Result<Option<NodeRow>> {
    Ok(sqlx::query_as(
        "SELECT n.* FROM nodes n JOIN node_tokens t ON t.node_id = n.id
         WHERE t.token_hash = ? AND t.revoked_at IS NULL",
    )
    .bind(hash)
    .fetch_optional(pool)
    .await?)
}

pub async fn touch_node(pool: &SqlitePool, id: &str, token_hash: &str) -> anyhow::Result<()> {
    let now = now_ts();
    sqlx::query("UPDATE nodes SET last_seen_at = ? WHERE id = ?")
        .bind(now)
        .bind(id)
        .execute(pool)
        .await?;
    sqlx::query("UPDATE node_tokens SET last_used_at = ? WHERE token_hash = ?")
        .bind(now)
        .bind(token_hash)
        .execute(pool)
        .await?;
    Ok(())
}

#[derive(Debug)]
pub struct InvalidJoinToken;

impl std::fmt::Display for InvalidJoinToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("join token inválido, expirado, revogado ou esgotado")
    }
}

impl std::error::Error for InvalidJoinToken {}

async fn acl_name_reserved(conn: &mut sqlx::SqliteConnection, name: &str) -> anyhow::Result<bool> {
    let row: Option<(String,)> = sqlx::query_as("SELECT yaml FROM acl_policies WHERE id = 1")
        .fetch_optional(&mut *conn).await?;
    let Some((yaml,)) = row else { return Ok(false) };
    let policy: AclPolicy = serde_yaml::from_str(&yaml)?;
    Ok(policy.rules.iter().any(|rule| {
        rule.src.iter().chain(&rule.dst).any(|selector| {
            selector.trim().strip_prefix("name:").is_some_and(|n| n.trim() == name)
        })
    }))
}

#[derive(Debug)]
pub struct AclNameReserved;

impl std::fmt::Display for AclNameReserved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("nome usado por ACL; alteração exige administrador")
    }
}
impl std::error::Error for AclNameReserved {}

/// Serialize a self-rename with ACL changes and reserve both source and target
/// names used as policy identities. Administrative rename is a separate API.
pub async fn rename_node_self(pool: &SqlitePool, id: Uuid, name: &str) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    // Acquire writer lock before inspecting policy/name to avoid a TOCTOU window.
    sqlx::query("UPDATE nodes SET name = name WHERE id = ?")
        .bind(id.to_string()).execute(&mut *tx).await?;
    let old: (String,) = sqlx::query_as("SELECT name FROM nodes WHERE id = ?")
        .bind(id.to_string()).fetch_one(&mut *tx).await?;
    if old.0 != name {
        if acl_name_reserved(&mut tx, &old.0).await? || acl_name_reserved(&mut tx, name).await? {
            return Err(AclNameReserved.into());
        }
        sqlx::query("UPDATE nodes SET name = ? WHERE id = ?")
            .bind(name).bind(id.to_string()).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Invitation consumption and node creation commit together. The first statement
/// acquires the SQLite writer lock, so concurrent enrollments cannot overuse it.
pub async fn enroll_node(
    pool: &SqlitePool,
    req: &wm_proto::EnrollRequest,
    token_hash: &str,
    node_token_hash: &str,
    status: NodeStatus,
    net: Ipv4Net,
) -> anyhow::Result<NodeRow> {
    let mut tx = pool.begin().await?;
    let token: JoinTokenRow = sqlx::query_as(
        "UPDATE join_tokens SET uses = uses + 1
         WHERE token_hash = ? AND revoked_at IS NULL AND expires_at >= ? AND uses < max_uses
         RETURNING *",
    )
    .bind(token_hash)
    .bind(now_ts())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(InvalidJoinToken)?;
    let tags = token.tags();
    let mut labels = req.labels.clone();
    // Invitation metadata is authoritative; clients cannot overwrite it.
    labels.extend(token.labels());
    if labels.len() > 64 {
        bail!("no máximo 64 labels");
    }
    // A client choosing a name already used as an ACL identity requires explicit
    // admin approval, even on networks otherwise allowing automatic enrollment.
    // An invitation may also require approval for every enrollment it creates.
    let status = if token.require_approval != 0 || acl_name_reserved(&mut tx, req.name.trim()).await? {
        NodeStatus::Pending
    } else {
        status
    };
    let node = create_node_in_transaction(
        &mut tx, req.name.trim(), req.hostname.trim(), &req.os, &req.arch,
        &req.version, &req.wg_public_key, &tags, &labels, status,
        &format!("join-token:{}", token.prefix), node_token_hash, net,
    ).await?;
    if !req.display_name.is_empty() || !req.about.is_empty() {
        sqlx::query("UPDATE nodes SET display_name = ?, about = ? WHERE id = ?")
            .bind(req.display_name.trim())
            .bind(req.about.trim())
            .bind(&node.id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(node_by_id(pool, node.uuid()).await?.unwrap_or(node))
}

/// Allocates the next free overlay address and creates the node plus its first
/// node token in a single transaction.
#[allow(clippy::too_many_arguments)]
pub async fn create_node(
    pool: &SqlitePool,
    name: &str,
    hostname: &str,
    os: &str,
    arch: &str,
    version: &str,
    wg_public_key: &str,
    tags: &[String],
    labels: &BTreeMap<String, String>,
    status: NodeStatus,
    created_by: &str,
    node_token_hash: &str,
    net: Ipv4Net,
) -> anyhow::Result<NodeRow> {
    let mut tx = pool.begin().await?;
    let node = create_node_in_transaction(
        &mut tx, name, hostname, os, arch, version, wg_public_key, tags,
        labels, status, created_by, node_token_hash, net,
    ).await?;
    tx.commit().await?;
    Ok(node)
}

#[allow(clippy::too_many_arguments)]
async fn create_node_in_transaction(
    tx: &mut sqlx::SqliteConnection,
    name: &str,
    hostname: &str,
    os: &str,
    arch: &str,
    version: &str,
    wg_public_key: &str,
    tags: &[String],
    labels: &BTreeMap<String, String>,
    status: NodeStatus,
    created_by: &str,
    node_token_hash: &str,
    net: Ipv4Net,
) -> anyhow::Result<NodeRow> {
    let used: Vec<(String,)> = sqlx::query_as("SELECT overlay_ip FROM nodes")
        .fetch_all(&mut *tx)
        .await?;
    let used: std::collections::HashSet<Ipv4Addr> = used
        .into_iter()
        .filter_map(|r| r.0.parse().ok())
        .collect();

    // .0 is the network, .1 is the server/gateway, broadcast is reserved.
    let server_ip = net.hosts().next().ok_or_else(|| anyhow!("rede inválida"))?;
    let broadcast = net.broadcast();
    let mut chosen = None;
    for ip in net.hosts() {
        if ip == server_ip || ip == broadcast || used.contains(&ip) {
            continue;
        }
        chosen = Some(ip);
        break;
    }
    let overlay_ip = chosen.ok_or_else(|| anyhow!("faixa de IPs do overlay esgotada"))?;

    let id = Uuid::new_v4();
    let state = match status {
        NodeStatus::Active => "active",
        NodeStatus::Disabled => "disabled",
        NodeStatus::Pending => "pending",
    };
    let now = now_ts();
    let approved = (status == NodeStatus::Active).then_some(now);
    let res = sqlx::query(
        "INSERT INTO nodes(id, name, hostname, os, arch, version, wg_public_key, overlay_ip, status, tags, labels, endpoints, created_at, approved_at, last_seen_at, created_by)
         VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(id.to_string())
    .bind(name)
    .bind(hostname)
    .bind(os)
    .bind(arch)
    .bind(version)
    .bind(wg_public_key)
    .bind(overlay_ip.to_string())
    .bind(state)
    .bind(serde_json::to_string(tags)?)
    .bind(serde_json::to_string(labels)?)
    .bind("[]")
    .bind(now)
    .bind(approved)
    .bind(Some(now))
    .bind(created_by)
    .execute(&mut *tx)
    .await;
    if let Err(err) = res {
        let msg = err.to_string();
        if msg.contains("UNIQUE") {
            bail!("nome, chave WireGuard ou IP já em uso");
        }
        return Err(err.into());
    }

    sqlx::query(
        "INSERT INTO node_tokens(id, node_id, token_hash, created_at) VALUES(?,?,?,?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(id.to_string())
    .bind(node_token_hash)
    .bind(now)
    .execute(&mut *tx)
    .await?;

    sqlx::query_as("SELECT * FROM nodes WHERE id = ?")
        .bind(id.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(Into::into)
}


pub async fn overlay_ip_in_use(pool: &SqlitePool, ip: &str) -> anyhow::Result<bool> {
    let row: Option<(String,)> = sqlx::query_as("SELECT id FROM nodes WHERE overlay_ip = ?")
        .bind(ip)
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

pub async fn set_node_overlay_ip(pool: &SqlitePool, id: Uuid, ip: &str) -> anyhow::Result<()> {
    sqlx::query("UPDATE nodes SET overlay_ip = ? WHERE id = ?")
        .bind(ip)
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn name_exists(pool: &SqlitePool, name: &str) -> anyhow::Result<bool> {
    let row: Option<(String,)> = sqlx::query_as("SELECT id FROM nodes WHERE name = ?")
        .bind(name)
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

pub async fn update_node_endpoints(
    pool: &SqlitePool,
    id: &str,
    endpoints: &[EndpointCandidate],
    nat_type: NatType,
    relay_connected: bool,
) -> anyhow::Result<bool> {
    let old: Option<(String,)> = sqlx::query_as("SELECT endpoints FROM nodes WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    let encoded = serde_json::to_string(endpoints)?;
    let changed = old.map(|r| r.0 != encoded).unwrap_or(true);
    sqlx::query(
        "UPDATE nodes SET endpoints = ?, nat_type = ?, relay_connected = ?, last_seen_at = ? WHERE id = ?",
    )
    .bind(&encoded)
    .bind(nat_type_str(nat_type))
    .bind(if relay_connected { 1i64 } else { 0i64 })
    .bind(now_ts())
    .bind(id)
    .execute(pool)
    .await?;
    Ok(changed)
}

pub async fn update_node_report(
    pool: &SqlitePool,
    id: &str,
    peers: &[PeerStats],
    nat_type: NatType,
    relay_connected: bool,
    version: &str,
    agent: &wm_proto::AgentRuntime,
) -> anyhow::Result<()> {
    let now = now_ts();
    sqlx::query(
        "UPDATE nodes SET reported_peers = ?, nat_type = ?, relay_connected = ?, last_seen_at = ?,
         version = CASE WHEN ? = '' THEN version ELSE ? END, agent_runtime = ? WHERE id = ?",
    )
    .bind(serde_json::to_string(peers)?)
    .bind(nat_type_str(nat_type))
    .bind(if relay_connected { 1i64 } else { 0i64 })
    .bind(now)
    .bind(version)
    .bind(version)
    .bind(serde_json::to_string(agent)?)
    .bind(id)
    .execute(pool)
    .await?;
    let mut tx = pool.begin().await?;
    for p in peers {
        sqlx::query(
            "INSERT INTO peer_stats(node_id, peer_id, path, rtt_ms, last_handshake_secs, rx_bytes, tx_bytes, relay_online, endpoint, last_error, updated_at)
             VALUES(?,?,?,?,?,?,?,?,?,?,?)
             ON CONFLICT(node_id, peer_id) DO UPDATE SET
               path=excluded.path, rtt_ms=excluded.rtt_ms, last_handshake_secs=excluded.last_handshake_secs,
               rx_bytes=excluded.rx_bytes, tx_bytes=excluded.tx_bytes, relay_online=excluded.relay_online,
               endpoint=excluded.endpoint, last_error=excluded.last_error, updated_at=excluded.updated_at",
        )
        .bind(id)
        .bind(p.peer_id.to_string())
        .bind(p.path.to_string())
        .bind(p.rtt_ms.map(i64::from))
        .bind(p.last_handshake_secs.map(|v| v as i64))
        .bind(p.rx_bytes as i64)
        .bind(p.tx_bytes as i64)
        .bind(i64::from(p.relay_online))
        .bind(&p.endpoint)
        .bind(&p.last_error)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

fn nat_type_str(n: NatType) -> &'static str {
    match n {
        NatType::Unknown => "unknown",
        NatType::Cone => "cone",
        NatType::Symmetric => "symmetric",
    }
}

/// Updates the presentation identity only. Callers must validate/normalize it.
pub async fn update_node_profile(
    pool: &SqlitePool,
    id: Uuid,
    display_name: &str,
    about: &str,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE nodes SET display_name = ?, about = ? WHERE id = ?")
        .bind(display_name)
        .bind(about)
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_node_requirements(
    pool: &SqlitePool,
    id: Uuid,
    requirements: &wm_proto::NodeRequirements,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE nodes SET requirements = ? WHERE id = ?")
        .bind(serde_json::to_string(requirements)?)
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_node_compliance(
    pool: &SqlitePool,
    id: Uuid,
    compliance: &wm_proto::NodeCompliance,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE nodes SET compliance = ? WHERE id = ?")
        .bind(serde_json::to_string(compliance)?)
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn update_node(
    pool: &SqlitePool,
    id: Uuid,
    name: Option<&str>,
    tags: Option<&[String]>,
    labels: Option<&BTreeMap<String, String>>,
    status: Option<NodeStatus>,
) -> anyhow::Result<()> {
    if let Some(name) = name {
        sqlx::query("UPDATE nodes SET name = ? WHERE id = ?")
            .bind(name)
            .bind(id.to_string())
            .execute(pool)
            .await?;
    }
    if let Some(tags) = tags {
        sqlx::query("UPDATE nodes SET tags = ? WHERE id = ?")
            .bind(serde_json::to_string(tags)?)
            .bind(id.to_string())
            .execute(pool)
            .await?;
    }
    if let Some(labels) = labels {
        sqlx::query("UPDATE nodes SET labels = ? WHERE id = ?")
            .bind(serde_json::to_string(labels)?)
            .bind(id.to_string())
            .execute(pool)
            .await?;
    }
    if let Some(status) = status {
        set_node_status(pool, id, status).await?;
    }
    Ok(())
}

pub async fn set_node_status(
    pool: &SqlitePool,
    id: Uuid,
    status: NodeStatus,
) -> anyhow::Result<()> {
    let now = now_ts();
    match status {
        NodeStatus::Active => {
            sqlx::query("UPDATE nodes SET status = 'active', approved_at = COALESCE(approved_at, ?), disabled_at = NULL WHERE id = ?")
                .bind(now)
                .bind(id.to_string())
                .execute(pool)
                .await?;
        }
        NodeStatus::Disabled => {
            sqlx::query("UPDATE nodes SET status = 'disabled', disabled_at = ? WHERE id = ?")
                .bind(now)
                .bind(id.to_string())
                .execute(pool)
                .await?;
            sqlx::query("UPDATE node_tokens SET revoked_at = ? WHERE node_id = ?")
                .bind(now)
                .bind(id.to_string())
                .execute(pool)
                .await?;
        }
        NodeStatus::Pending => {
            sqlx::query("UPDATE nodes SET status = 'pending', approved_at = NULL WHERE id = ?")
                .bind(id.to_string())
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

pub struct AclRow {
    pub version: i64,
    pub yaml: String,
    pub compiled: String,
    pub updated_by: Option<String>,
    pub updated_at: i64,
}

impl_from_row!(AclRow { version, yaml, compiled, updated_by, updated_at });

pub async fn get_acl_row(pool: &SqlitePool) -> anyhow::Result<Option<AclRow>> {
    Ok(sqlx::query_as("SELECT version, yaml, compiled, updated_by, updated_at FROM acl_policies WHERE id = 1")
        .fetch_optional(pool)
        .await?)
}

pub async fn store_acl(
    pool: &SqlitePool,
    policy: &AclPolicy,
    compiled: &wm_proto::CompiledAcl,
    updated_by: &str,
) -> anyhow::Result<()> {
    let yaml = serde_yaml::to_string(policy)?;
    sqlx::query(
        "INSERT INTO acl_policies(id, version, yaml, compiled, updated_by, updated_at)
         VALUES(1,?,?,?,?,?)
         ON CONFLICT(id) DO UPDATE SET version=excluded.version, yaml=excluded.yaml,
         compiled=excluded.compiled, updated_by=excluded.updated_by, updated_at=excluded.updated_at",
    )
    .bind(i64::from(policy.version))
    .bind(yaml)
    .bind(serde_json::to_string(compiled)?)
    .bind(updated_by)
    .bind(now_ts())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn acl_view(pool: &SqlitePool) -> anyhow::Result<AclView> {
    match get_acl_row(pool).await? {
        Some(row) => {
            let policy: AclPolicy = serde_yaml::from_str(&row.yaml)
                .context("ACL persistida inválida")?;
            let compiled: wm_proto::CompiledAcl = serde_json::from_str(&row.compiled)
                .context("ACL compilada persistida inválida")?;
            Ok(AclView {
                policy,
                compiled,
                updated_at: Some(ts_to_dt(row.updated_at)),
            })
        }
        None => Ok(AclView {
            policy: AclPolicy::default(),
            compiled: wm_proto::CompiledAcl::default(),
            updated_at: None,
        }),
    }
}

pub struct ServiceTokenRow {
    pub id: String,
    pub name: String,
    pub scopes: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

impl_from_row!(ServiceTokenRow {
    id,
    name,
    scopes,
    created_at,
    last_used_at,
    revoked_at
});

impl ServiceTokenRow {
    pub fn scopes(&self) -> Vec<String> {
        serde_json::from_str(&self.scopes).unwrap_or_default()
    }
}

pub async fn create_service_token(
    pool: &SqlitePool,
    name: &str,
    hash: &str,
    scopes: &[String],
) -> anyhow::Result<ServiceTokenRow> {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO service_tokens(id, name, token_hash, scopes, created_at) VALUES(?,?,?,?,?)",
    )
    .bind(id.to_string())
    .bind(name)
    .bind(hash)
    .bind(serde_json::to_string(scopes)?)
    .bind(now_ts())
    .execute(pool)
    .await?;
    Ok(ServiceTokenRow {
        id: id.to_string(),
        name: name.into(),
        scopes: serde_json::to_string(scopes)?,
        created_at: now_ts(),
        last_used_at: None,
        revoked_at: None,
    })
}

pub async fn service_token_by_hash(
    pool: &SqlitePool,
    hash: &str,
) -> anyhow::Result<Option<ServiceTokenRow>> {
    Ok(sqlx::query_as(
        "SELECT id, name, scopes, created_at, last_used_at, revoked_at
         FROM service_tokens WHERE token_hash = ? AND revoked_at IS NULL",
    )
    .bind(hash)
    .fetch_optional(pool)
    .await?)
}

pub async fn touch_service_token(pool: &SqlitePool, id: &str) -> anyhow::Result<()> {
    sqlx::query("UPDATE service_tokens SET last_used_at = ? WHERE id = ?")
        .bind(now_ts())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn list_service_tokens(pool: &SqlitePool) -> anyhow::Result<Vec<ServiceTokenRow>> {
    Ok(sqlx::query_as(
        "SELECT id, name, scopes, created_at, last_used_at, revoked_at
         FROM service_tokens ORDER BY created_at DESC",
    )
    .fetch_all(pool)
    .await?)
}

pub async fn revoke_service_token(pool: &SqlitePool, id: Uuid) -> anyhow::Result<()> {
    sqlx::query("UPDATE service_tokens SET revoked_at = ? WHERE id = ?")
        .bind(now_ts())
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn audit(
    pool: &SqlitePool,
    actor: &str,
    action: &str,
    target: &str,
    detail: Option<&str>,
    ip: Option<&str>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO audit_events(ts, actor, action, target, detail, ip) VALUES(?,?,?,?,?,?)",
    )
    .bind(now_ts())
    .bind(actor)
    .bind(action)
    .bind(target)
    .bind(detail)
    .bind(ip)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_audit(pool: &SqlitePool, limit: i64) -> anyhow::Result<Vec<wm_proto::AuditEventView>> {
    let rows: Vec<(
        i64,
        i64,
        String,
        String,
        String,
        Option<String>,
        Option<String>,
    )> = sqlx::query_as(
        "SELECT id, ts, actor, action, target, detail, ip FROM audit_events ORDER BY id DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, ts, actor, action, target, detail, ip)| wm_proto::AuditEventView {
            id,
            ts: ts_to_dt(ts),
            actor,
            action,
            target,
            detail,
            ip,
        })
        .collect())
}

pub async fn update_service_token_usage(pool: &SqlitePool, id: &str) -> anyhow::Result<()> {
    touch_service_token(pool, id).await
}

// ---------------------------------------------------------------------------
// Shares
// ---------------------------------------------------------------------------

pub struct ShareRow {
    pub id: String,
    pub name: String,
    pub node_id: String,
    pub port: i64,
    pub kind: String,
    pub protocol: String,
    pub description: String,
    pub enabled: i64,
    pub public_url: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl_from_row!(ShareRow {
    id,
    name,
    node_id,
    port,
    kind,
    protocol,
    description,
    enabled,
    public_url,
    created_at,
    updated_at
});

impl ShareRow {
    pub fn uuid(&self) -> Uuid {
        Uuid::parse_str(&self.id).unwrap_or_else(|_| Uuid::new_v4())
    }

    pub fn node_uuid(&self) -> Uuid {
        Uuid::parse_str(&self.node_id).unwrap_or_else(|_| Uuid::new_v4())
    }

    pub fn port(&self) -> u16 {
        u16::try_from(self.port).unwrap_or(0)
    }

    pub fn kind(&self) -> wm_proto::ShareKind {
        wm_proto::ShareKind::parse(&self.kind).unwrap_or_default()
    }

    pub fn protocol(&self) -> wm_proto::ShareProtocol {
        wm_proto::ShareProtocol::parse(&self.protocol).unwrap_or_default()
    }

    pub fn enabled(&self) -> bool {
        self.enabled != 0
    }

    pub fn info(&self, node: &NodeRow, now: i64) -> wm_proto::ShareInfo {
        wm_proto::ShareInfo {
            id: self.uuid(),
            name: self.name.clone(),
            node_id: self.node_uuid(),
            node_name: node.name.clone(),
            overlay_ip: node.overlay_ip(),
            port: self.port(),
            kind: self.kind(),
            protocol: self.protocol(),
            description: self.description.clone(),
            enabled: self.enabled(),
            online: node.online(now),
            public_url: self.public_url.clone(),
            created_at: ts_to_dt(self.created_at),
            updated_at: ts_to_dt(self.updated_at),
        }
    }
}

pub async fn list_shares(pool: &SqlitePool) -> anyhow::Result<Vec<ShareRow>> {
    Ok(sqlx::query_as("SELECT * FROM shares ORDER BY name")
        .fetch_all(pool)
        .await?)
}

pub async fn list_shares_for_node(
    pool: &SqlitePool,
    node_id: Uuid,
) -> anyhow::Result<Vec<ShareRow>> {
    Ok(sqlx::query_as("SELECT * FROM shares WHERE node_id = ? ORDER BY name")
        .bind(node_id.to_string())
        .fetch_all(pool)
        .await?)
}

pub async fn share_by_id(pool: &SqlitePool, id: Uuid) -> anyhow::Result<Option<ShareRow>> {
    Ok(sqlx::query_as("SELECT * FROM shares WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(pool)
        .await?)
}

pub async fn share_by_name(pool: &SqlitePool, name: &str) -> anyhow::Result<Option<ShareRow>> {
    Ok(sqlx::query_as("SELECT * FROM shares WHERE name = ?")
        .bind(name)
        .fetch_optional(pool)
        .await?)
}

pub async fn share_name_taken(
    pool: &SqlitePool,
    name: &str,
    except: Option<Uuid>,
) -> anyhow::Result<bool> {
    let row: Option<(String,)> = sqlx::query_as("SELECT id FROM shares WHERE name = ?")
        .bind(name)
        .fetch_optional(pool)
        .await?;
    Ok(row
        .map(|r| Some(r.0.as_str()) != except.map(|id| id.to_string()).as_deref())
        .unwrap_or(false))
}

#[allow(clippy::too_many_arguments)]
pub async fn create_share(
    pool: &SqlitePool,
    name: &str,
    node_id: Uuid,
    port: u16,
    kind: wm_proto::ShareKind,
    protocol: wm_proto::ShareProtocol,
    description: &str,
    enabled: bool,
) -> anyhow::Result<ShareRow> {
    let id = Uuid::new_v4();
    let now = now_ts();
    sqlx::query(
        "INSERT INTO shares(id, name, node_id, port, kind, protocol, description, enabled, public_url, created_at, updated_at)
         VALUES(?,?,?,?,?,?,?,?,NULL,?,?)",
    )
    .bind(id.to_string())
    .bind(name)
    .bind(node_id.to_string())
    .bind(i64::from(port))
    .bind(kind.as_str())
    .bind(protocol.as_str())
    .bind(description)
    .bind(i64::from(enabled))
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|err| anyhow!("não foi possível criar o share: {err}"))?;
    share_by_id(pool, id)
        .await?
        .ok_or_else(|| anyhow!("share criado mas não encontrado"))
}

#[allow(clippy::too_many_arguments)]
pub async fn update_share(
    pool: &SqlitePool,
    id: Uuid,
    name: &str,
    port: u16,
    kind: wm_proto::ShareKind,
    protocol: wm_proto::ShareProtocol,
    description: &str,
    enabled: bool,
) -> anyhow::Result<Option<ShareRow>> {
    let result = sqlx::query(
        "UPDATE shares SET name = ?, port = ?, kind = ?, protocol = ?, description = ?, enabled = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(name)
    .bind(i64::from(port))
    .bind(kind.as_str())
    .bind(protocol.as_str())
    .bind(description)
    .bind(i64::from(enabled))
    .bind(now_ts())
    .bind(id.to_string())
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Ok(None);
    }
    share_by_id(pool, id).await
}

pub async fn delete_share(pool: &SqlitePool, id: Uuid) -> anyhow::Result<Option<ShareRow>> {
    let row = share_by_id(pool, id).await?;
    if row.is_some() {
        sqlx::query("DELETE FROM shares WHERE id = ?")
            .bind(id.to_string())
            .execute(pool)
            .await?;
    }
    Ok(row)
}

pub async fn set_share_public_url(
    pool: &SqlitePool,
    id: Uuid,
    public_url: Option<&str>,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE shares SET public_url = ?, updated_at = ? WHERE id = ?")
        .bind(public_url)
        .bind(now_ts())
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn schema_and_settings() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open(&dir.path().join("t.db")).await.unwrap();
        set_setting(&pool, "x", "y").await.unwrap();
        assert_eq!(get_setting(&pool, "x").await.unwrap().as_deref(), Some("y"));
        assert!(ensure_admin(&pool, "admin", Some("secret"))
            .await
            .unwrap()
            .is_none());
        assert!(admin_by_username(&pool, "admin").await.unwrap().is_some());
    }
}
