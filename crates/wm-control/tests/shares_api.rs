//! Control-plane test for the share registry: node self-service, map
//! projection, duplicate-name conflict and node rename.

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use tower::ServiceExt;

use wm_control::auth;
use wm_control::config::ControlConfig;
use wm_control::db;
use wm_control::state::AppState;
use wm_proto::NodeStatus;

async fn boot() -> (tempfile::TempDir, Arc<AppState>) {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = ControlConfig::default();
    cfg.data_dir = dir.path().join("control");
    cfg.public_url = "http://127.0.0.1:18090".into();
    cfg.admin_password = Some("secret".into());
    let (state, generated) = AppState::new(cfg).await.unwrap();
    assert!(generated.is_none());
    (dir, state)
}

struct Admin {
    cookie: String,
    csrf: String,
}

async fn login(app: &axum::Router) -> Admin {
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/v1/admin/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"username": "admin", "password": "secret"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    Admin {
        cookie,
        csrf: body["csrf_token"].as_str().unwrap().to_string(),
    }
}

async fn request(
    app: &axum::Router,
    method: &str,
    path: &str,
    admin: Option<&Admin>,
    bearer: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(admin) = admin {
        builder = builder
            .header(header::COOKIE, &admin.cookie)
            .header("x-wm-csrf", &admin.csrf);
    }
    if let Some(token) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let body = match body {
        Some(value) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(value.to_string())
        }
        None => Body::empty(),
    };
    let response = app.clone().oneshot(builder.body(body).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

#[tokio::test]
async fn share_lifecycle_reaches_map_and_admin() {
    let (_dir, state) = boot().await;
    let app = wm_control::api::router(state.clone());
    let admin = login(&app).await;

    let node_token = auth::random_token("wmn_").unwrap();
    let wg_key = STANDARD.encode([9u8; 32]);
    let node = db::create_node(
        &state.db,
        "dev-a",
        "dev-a.local",
        "macos",
        "aarch64",
        "0.1.0",
        &wg_key,
        &[],
        &Default::default(),
        NodeStatus::Active,
        "test",
        &auth::hash_token(&node_token),
        state.overlay,
    )
    .await
    .unwrap();

    // Create through the node's own bearer token.
    let (status, share) = request(
        &app,
        "POST",
        "/api/v1/node/shares",
        None,
        Some(&node_token),
        Some(json!({
            "name": "Demo",
            "port": 3000,
            "kind": "web",
            "protocol": "both",
            "description": "preview",
            "enabled": true
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{share}");
    assert_eq!(share["name"], "demo");
    assert_eq!(share["protocol"], "tcp", "web is always tcp");
    assert_eq!(share["node_name"], "dev-a");
    assert_eq!(share["overlay_ip"], node.overlay_ip().to_string());
    let id = share["id"].as_str().unwrap().to_string();

    // The share is in the map for the owner.
    let (status, map) = request(
        &app,
        "GET",
        "/api/v1/map?version=0&wait=1",
        None,
        Some(&node_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{map}");
    assert_eq!(map["shares"][0]["name"], "demo");
    assert_eq!(map["shares"][0]["port"], 3000);

    // A second node cannot take the same name.
    let other_token = auth::random_token("wmn_").unwrap();
    let other = db::create_node(
        &state.db,
        "dev-b",
        "dev-b.local",
        "linux",
        "x86_64",
        "0.1.0",
        &STANDARD.encode([8u8; 32]),
        &[],
        &Default::default(),
        NodeStatus::Active,
        "test",
        &auth::hash_token(&other_token),
        state.overlay,
    )
    .await
    .unwrap();
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/node/shares",
        None,
        Some(&other_token),
        Some(json!({"name": "demo", "port": 3001, "kind": "tcp"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "share_name_taken");

    // Only the owner can edit it.
    let (status, _) = request(
        &app,
        "PATCH",
        &format!("/api/v1/node/shares/{id}"),
        None,
        Some(&other_token),
        Some(json!({"enabled": false})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The owner disables and renames it.
    let (status, updated) = request(
        &app,
        "PATCH",
        &format!("/api/v1/node/shares/{id}"),
        None,
        Some(&node_token),
        Some(json!({"name": "demo-v2", "enabled": false})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["name"], "demo-v2");
    assert_eq!(updated["enabled"], false);

    // Disabled shares remain in the map so the owner can toggle them.
    let (_, map) = request(
        &app,
        "GET",
        "/api/v1/map?version=0&wait=1",
        None,
        Some(&node_token),
        None,
    )
    .await;
    assert_eq!(map["shares"][0]["name"], "demo-v2");
    assert_eq!(map["shares"][0]["enabled"], false);

    // Admin sees the share.
    let (status, list) = request(&app, "GET", "/api/v1/admin/shares", Some(&admin), None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list[0]["name"], "demo-v2");

    // Rename the node; the map reflects it on the next poll.
    let (status, renamed) = request(
        &app,
        "PATCH",
        "/api/v1/node/self",
        None,
        Some(&node_token),
        Some(json!({"name": "dev-a-renamed"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    assert_eq!(renamed["name"], "dev-a-renamed");
    let (_, map) = request(
        &app,
        "GET",
        "/api/v1/map?version=0&wait=1",
        None,
        Some(&node_token),
        None,
    )
    .await;
    assert_eq!(map["me"]["name"], "dev-a-renamed");

    // Delete it: gone from the admin list and from the map.
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/v1/node/shares/{id}"),
        None,
        Some(&node_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, list) = request(&app, "GET", "/api/v1/admin/shares", Some(&admin), None, None).await;
    assert!(list.as_array().unwrap().is_empty());
    let (_, map) = request(
        &app,
        "GET",
        "/api/v1/map?version=0&wait=1",
        None,
        Some(&node_token),
        None,
    )
    .await;
    assert!(map["shares"].as_array().unwrap().is_empty());
    let _ = other.uuid();
}

#[tokio::test]
async fn installer_serves_script_and_rejects_traversal() {
    let (_dir, state) = boot().await;
    let app = wm_control::api::router(state.clone());
    let (status, _) = request(&app, "GET", "/j/wmj_abc-123", None, None, None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = request(&app, "GET", "/j/..%2Fetc%2Fpasswd", None, None, None).await;
    assert_ne!(status, StatusCode::OK);
}
