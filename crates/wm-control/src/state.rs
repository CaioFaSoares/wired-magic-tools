use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;
use chrono::Utc;
use ed25519_dalek::SigningKey;
use ipnet::Ipv4Net;
use sqlx::SqlitePool;
use tokio::sync::{broadcast, watch};

use wm_proto::{CompiledAcl, ControlEvent, NetworkInfo, NodeId, RelayClaims, RelayInfo};

use crate::auth::{self, RateLimiter};
use crate::config::ControlConfig;
use crate::db;
use crate::extensions::ExtensionRegistry;
use crate::relay::RelayHub;

pub struct AppState {
    pub cfg: ControlConfig,
    pub db: SqlitePool,
    pub started: Instant,
    pub version: watch::Sender<u64>,
    pub events: broadcast::Sender<ControlEvent>,
    pub relay: RelayHub,
    pub extensions: Arc<ExtensionRegistry>,
    pub signing_key: Arc<SigningKey>,
    pub control_pubkey: String,
    pub network: NetworkInfo,
    pub overlay: Ipv4Net,
    pub login_limiter: RateLimiter,
    pub enroll_limiter: RateLimiter,
    pub presence: Mutex<HashMap<NodeId, bool>>,
}

impl AppState {
    pub async fn new(mut cfg: ControlConfig) -> anyhow::Result<(Arc<Self>, Option<String>)> {
        cfg.ensure_dirs()?;
        cfg.validate().map_err(|e| anyhow::anyhow!(e))?;
        let db = db::open(&cfg.db_path())
            .await
            .with_context(|| format!("abrindo banco {}", cfg.db_path().display()))?;

        // The signing key authenticates relay tokens. It is generated once and
        // persisted; losing it invalidates outstanding relay tokens until the
        // next map refresh, which is harmless.
        let seed = db::get_setting(&db, "control_signing_key").await?;
        let signing_key = match seed {
            Some(ref seed) => auth::signing_key_from_b64(seed)?,
            None => {
                let key = auth::new_control_signing_key();
                db::set_setting(&db, "control_signing_key", &auth::signing_key_to_b64(&key)).await?;
                key
            }
        };
        let control_pubkey = auth::public_key_b64(&signing_key);

        let generated_pw = db::ensure_admin(
            &db,
            "admin",
            cfg.admin_password.as_deref(),
        )
        .await?;

        // Extensions are loaded from disk once the state directory exists. A
        // bad manifest becomes a warning, never a boot failure.
        let extensions = Arc::new(ExtensionRegistry::load(&cfg.data_dir)?);

        let version = db::get_setting(&db, "map_version")
            .await?
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(1);
        let (version_tx, _) = watch::channel(version.max(1));
        let (events_tx, _) = broadcast::channel(512);
        let overlay: Ipv4Net = cfg.overlay_cidr.parse()?;

        let state = Arc::new(Self {
            network: cfg.network_info(),
            cfg,
            db,
            started: Instant::now(),
            version: version_tx,
            events: events_tx,
            relay: RelayHub::new(),
            extensions,
            signing_key: Arc::new(signing_key),
            control_pubkey,
            overlay,
            login_limiter: RateLimiter::new(10, Duration::from_secs(60)),
            enroll_limiter: RateLimiter::new(60, Duration::from_secs(60)),
            presence: Mutex::new(HashMap::new()),
        });

        // Persist the initial version so restarts never reuse an old map number.
        let _ = db::set_setting(&state.db, "map_version", &state.current_version().to_string()).await;
        Ok((state, generated_pw))
    }

    pub fn current_version(&self) -> u64 {
        *self.version.borrow()
    }

    pub fn subscribe_version(&self) -> watch::Receiver<u64> {
        self.version.subscribe()
    }

    pub fn bump_version(&self) {
        let next = self.current_version().wrapping_add(1);
        let _ = self.version.send(next);
        tracing::debug!(version = next, "map version atualizada");
    }

    pub fn broadcast(&self, event: ControlEvent) {
        let _ = self.events.send(event);
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<ControlEvent> {
        self.events.subscribe()
    }

    pub fn relay_audience(&self) -> String {
        self.cfg.relay_public_url()
    }

    pub fn relay_info_for(&self, node: NodeId) -> Option<RelayInfo> {
        if !self.cfg.relay_enabled {
            return None;
        }
        let token = auth::sign_relay_token(
            &self.signing_key,
            node,
            &self.relay_audience(),
            3600,
        )
        .ok()?;
        Some(RelayInfo {
            url: self.cfg.relay_public_url(),
            region: self.cfg.relay_region.clone(),
            token,
            expires_at: Utc::now() + chrono::Duration::seconds(3600),
        })
    }

    pub fn relay_claims_for(&self, node: NodeId) -> Option<RelayClaims> {
        self.relay_info_for(node).map(|info| RelayClaims {
            sub: node,
            aud: info.url,
            iat: Utc::now().timestamp(),
            exp: info.expires_at.timestamp(),
        })
    }

    pub async fn compiled_acl(&self) -> anyhow::Result<CompiledAcl> {
        Ok(db::acl_view(&self.db).await?.compiled)
    }

    pub fn is_https(&self) -> bool {
        self.cfg.public_url.starts_with("https://")
    }

    pub async fn node_online(&self, row: &db::NodeRow) -> bool {
        row.online(db::now_ts())
            || self.relay.is_online(row.uuid())
    }

    pub async fn mark_presence(&self, node: NodeId, online: bool) -> Option<bool> {
        let mut map = self.presence.lock().ok()?;
        let previous = map.insert(node, online);
        if previous != Some(online) {
            Some(online)
        } else {
            None
        }
    }

    pub async fn shutdown(&self) {
        let _ = self.db.close().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[tokio::test]
    async fn boots_and_signs_relay_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = ControlConfig::default();
        cfg.data_dir = dir.path().to_path_buf();
        cfg.public_url = "http://127.0.0.1:18080".into();
        let (state, pw) = AppState::new(cfg).await.unwrap();
        assert!(pw.is_some());
        let node = Uuid::new_v4();
        let info = state.relay_info_for(node).unwrap();
        let claims =
            auth::verify_relay_token(&state.control_pubkey, &info.token, &state.relay_audience())
                .unwrap();
        assert_eq!(claims.sub, node);
    }
}
