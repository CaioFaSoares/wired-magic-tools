use std::collections::BTreeMap;

use wm_control::db;
use wm_proto::{EnrollRequest, NodeStatus};

fn request(name: &str, key: &str) -> EnrollRequest {
    serde_json::from_value(serde_json::json!({
        "join_token": "not-used-by-db", "name": name, "hostname": name,
        "os": "linux", "arch": "x86_64", "version": "test",
        "wg_public_key": key, "tags": ["site"],
        "labels": {"tenant": "attacker", "description": "client"}
    })).unwrap()
}

async fn invite(pool: &sqlx::SqlitePool, hash: &str) {
    db::create_join_token(
        pool, "test", "prefix", hash, &["mac".into()],
        &BTreeMap::from([("tenant".into(), "owner".into())]),
        1, 3600, "admin:test",
    ).await.unwrap();
}

#[tokio::test]
async fn failed_creation_rolls_back_invitation_and_preserves_authority() {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::open(&dir.path().join("test.db")).await.unwrap();
    let net = "10.66.0.0/24".parse().unwrap();
    invite(&pool, "first").await;
    db::enroll_node(&pool, &request("first", "key"), "first", "node-first", NodeStatus::Active, net)
        .await.unwrap();
    invite(&pool, "retry").await;
    assert!(db::enroll_node(&pool, &request("second", "key"), "retry", "node-second", NodeStatus::Active, net)
        .await.is_err());
    let uses: (i64,) = sqlx::query_as("SELECT uses FROM join_tokens WHERE token_hash = 'retry'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(uses.0, 0);
    let node = db::enroll_node(&pool, &request("second", "different-key"), "retry", "node-second", NodeStatus::Active, net)
        .await.unwrap();
    assert_eq!(node.tags(), vec!["mac"]);
    assert_eq!(node.labels()["tenant"], "owner");
    assert_eq!(node.labels()["description"], "client");
    assert_eq!(db::list_nodes(&pool).await.unwrap().len(), 2);
    let tokens: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM node_tokens")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(tokens.0, 2);
}

#[tokio::test]
async fn corrupted_persisted_acl_does_not_become_default_allow() {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::open(&dir.path().join("test.db")).await.unwrap();
    db::store_acl(&pool, &wm_proto::AclPolicy::default(), &wm_proto::CompiledAcl::default(), "admin:test")
        .await.unwrap();
    sqlx::query("UPDATE acl_policies SET compiled = 'not-json'").execute(&pool).await.unwrap();
    assert!(db::acl_view(&pool).await.is_err());
    db::store_acl(&pool, &wm_proto::AclPolicy::default(), &wm_proto::CompiledAcl::default(), "admin:test")
        .await.unwrap();
    sqlx::query("UPDATE acl_policies SET yaml = '['").execute(&pool).await.unwrap();
    assert!(db::acl_view(&pool).await.is_err());
    invite(&pool, "corrupt").await;
    assert!(db::enroll_node(&pool, &request("safe", "safe-key"), "corrupt", "node-safe", NodeStatus::Active, "10.66.0.0/24".parse().unwrap())
        .await.is_err());
    let uses: (i64,) = sqlx::query_as("SELECT uses FROM join_tokens WHERE token_hash = 'corrupt'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(uses.0, 0);
}

#[tokio::test]
async fn acl_names_cannot_be_self_assigned_or_released() {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::open(&dir.path().join("test.db")).await.unwrap();
    let policy = serde_yaml::from_str(
        "version: 1\nrules:\n  - action: accept\n    src: ['name:site']\n    dst: ['*']\n",
    ).unwrap();
    db::store_acl(&pool, &policy, &wm_proto::CompiledAcl::default(), "admin:test")
        .await.unwrap();
    let net = "10.66.0.0/24".parse().unwrap();
    invite(&pool, "reserved").await;
    let reserved = db::enroll_node(&pool, &request("site", "site-key"), "reserved", "node-site", NodeStatus::Active, net)
        .await.unwrap();
    assert_eq!(reserved.status(), NodeStatus::Pending);
    invite(&pool, "ordinary").await;
    let ordinary = db::enroll_node(&pool, &request("ordinary", "ordinary-key"), "ordinary", "node-ordinary", NodeStatus::Active, net)
        .await.unwrap();
    assert_eq!(ordinary.status(), NodeStatus::Active);
    let err = db::rename_node_self(&pool, reserved.uuid(), "released").await.err().unwrap();
    assert!(err.downcast_ref::<db::AclNameReserved>().is_some());
    // Remove the placeholder so a unique-name conflict cannot hide the ACL guard.
    db::update_node(&pool, reserved.uuid(), Some("admin-renamed"), None, None, None).await.unwrap();
    let err = db::rename_node_self(&pool, ordinary.uuid(), "site").await.err().unwrap();
    assert!(err.downcast_ref::<db::AclNameReserved>().is_some());
    assert_eq!(db::node_by_id(&pool, ordinary.uuid()).await.unwrap().unwrap().name, "ordinary");
    db::rename_node_self(&pool, ordinary.uuid(), "friendly").await.unwrap();
}

#[tokio::test]
async fn concurrent_single_use_enrollment_has_exactly_one_winner() {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::open(&dir.path().join("test.db")).await.unwrap();
    invite(&pool, "once").await;
    let net = "10.66.0.0/24".parse().unwrap();
    let a = request("a", "a-key");
    let b = request("b", "b-key");
    let (a, b) = tokio::join!(
        db::enroll_node(&pool, &a, "once", "node-a", NodeStatus::Active, net),
        db::enroll_node(&pool, &b, "once", "node-b", NodeStatus::Active, net),
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let loser = match (a, b) {
        (Err(e), Ok(_)) | (Ok(_), Err(e)) => e,
        _ => panic!("expected exactly one successful enrollment"),
    };
    assert!(loser.downcast_ref::<db::InvalidJoinToken>().is_some(), "{loser:#}");
    assert_eq!(db::list_nodes(&pool).await.unwrap().len(), 1);
    let uses: (i64,) = sqlx::query_as("SELECT uses FROM join_tokens WHERE token_hash = 'once'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(uses.0, 1);
}
