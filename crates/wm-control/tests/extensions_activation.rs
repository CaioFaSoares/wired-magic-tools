//! Activation, catalog and declarative adapter materialization.

use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use wm_control::auth;
use wm_control::config::ControlConfig;
use wm_control::db;
use wm_control::state::AppState;
use wm_proto::NodeStatus;

async fn boot() -> (tempfile::TempDir, Arc<AppState>) {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = ControlConfig::default();
    cfg.data_dir = dir.path().join("control");
    cfg.public_url = "http://127.0.0.1:18190".into();
    cfg.admin_password = Some("secret".into());
    let (state, _) = AppState::new(cfg).await.unwrap();
    (dir, state)
}

async fn node(state: &AppState, name: &str) -> db::NodeRow {
    let token = auth::random_token("wmn_").unwrap();
    db::create_node(
        &state.db,
        name,
        &format!("{name}.local"),
        "linux",
        "x86_64",
        "0.1.0",
        &STANDARD.encode([9u8; 32]),
        &[],
        &Default::default(),
        NodeStatus::Active,
        "test",
        &auth::hash_token(&token),
        state.overlay,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn cloudflared_is_inactive_until_activated() {
    let (_dir, state) = boot().await;
    let node = node(&state, "mac-inactive").await;
    let catalog = state.extensions.node_catalog(&state, &node).await.unwrap();
    let cloudflared = catalog
        .iter()
        .find(|view| view.id == "cloudflared")
        .expect("cloudflared catalog entry");
    assert!(!cloudflared.active);

    let activated = state
        .extensions
        .set_node_activation(&state, &node, "cloudflared", true, "test")
        .await
        .unwrap()
        .expect("view");
    assert!(activated.active);
    assert!(activated.activation_revision >= 1);
    let map = wm_control::api::node::build_map(&state, &node).await.unwrap();
    assert!(map.extensions.iter().any(|desired| desired.id == "cloudflared"));

    state
        .extensions
        .set_node_activation(&state, &node, "cloudflared", false, "test")
        .await
        .unwrap();
    let map = wm_control::api::node::build_map(&state, &node).await.unwrap();
    assert!(!map.extensions.iter().any(|desired| desired.id == "cloudflared"));
}

#[tokio::test]
async fn seance_adapter_materializes_share_labels_and_exposes() {
    let (_dir, state) = boot().await;
    let node = node(&state, "mac-seance").await;

    let view = state
        .extensions
        .set_node_activation(&state, &node, "seance", true, "test")
        .await
        .unwrap()
        .expect("seance view");
    assert!(view.active);
    assert!(view.configured, "default_config should be materialized");

    let current = db::node_by_id(&state.db, node.uuid())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.labels().get("seance").map(String::as_str), Some("true"));
    assert_eq!(
        current.labels().get("baguette_port").map(String::as_str),
        Some("8421")
    );

    let share = db::share_by_name(&state.db, "baguette-mac-seance")
        .await
        .unwrap()
        .expect("adapter share");
    assert_eq!(share.port(), 8421);
    assert_eq!(share.node_uuid(), node.uuid());

    let exposes = state.extensions.exposes_for(&state, &node).await;
    assert!(exposes.iter().any(|expose| expose.overlay_port == 8421));

    // Revoking the node removes adapter-owned network effects even if its
    // activation row is still true.
    db::set_node_status(&state.db, node.uuid(), NodeStatus::Disabled)
        .await
        .unwrap();
    wm_control::extensions::adapters::sync_node(&state, &node)
        .await
        .unwrap();
    assert!(db::share_by_name(&state.db, "baguette-mac-seance")
        .await
        .unwrap()
        .is_none());
    let revoked = db::node_by_id(&state.db, node.uuid())
        .await
        .unwrap()
        .unwrap();
    assert!(revoked.labels().get("seance").is_none());
    db::set_node_status(&state.db, node.uuid(), NodeStatus::Active)
        .await
        .unwrap();
    wm_control::extensions::adapters::sync_node(&state, &node)
        .await
        .unwrap();
    assert!(db::share_by_name(&state.db, "baguette-mac-seance")
        .await
        .unwrap()
        .is_some());

    let map = wm_control::api::node::build_map(&state, &node).await.unwrap();
    assert!(map.exposes.iter().any(|expose| expose.overlay_port == 8421));
    assert!(map.shares.iter().any(|entry| entry.name == "baguette-mac-seance"));
    let desired = map
        .extensions
        .iter()
        .find(|desired| desired.id == "seance")
        .expect("seance desired state");
    assert_eq!(desired.driver, "exec");
    assert!(desired.hooks.install.is_some());
    assert_eq!(desired.config["command"], "/usr/local/bin/seance-agent");

    state
        .extensions
        .set_node_activation(&state, &node, "seance", false, "test")
        .await
        .unwrap();
    let current = db::node_by_id(&state.db, node.uuid())
        .await
        .unwrap()
        .unwrap();
    assert!(current.labels().get("seance").is_none());
    assert!(db::share_by_name(&state.db, "baguette-mac-seance")
        .await
        .unwrap()
        .is_none());
    assert!(state.extensions.exposes_for(&state, &node).await.is_empty());
}
