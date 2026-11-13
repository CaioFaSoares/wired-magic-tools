//! End-to-end-ish control-plane extension test: admin login, node settings,
//! route creation, map delivery and report round-trip, all through the axum
//! router (no sockets, no real agents).

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use wm_control::auth;
use wm_control::config::ControlConfig;
use wm_control::db;
use wm_control::state::AppState;
use wm_proto::{NetworkMap, NodeStatus};

async fn boot() -> (tempfile::TempDir, Arc<AppState>) {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = ControlConfig::default();
    cfg.data_dir = dir.path().join("control");
    cfg.public_url = "http://127.0.0.1:18080".into();
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
    assert!(cookie.starts_with(auth::SESSION_COOKIE));
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
async fn cloudflared_route_reaches_map_and_status_roundtrips() {
    let (_dir, state) = boot().await;
    let app = wm_control::api::router(state.clone());
    let admin = login(&app).await;

    // The extension host seeded the built-in manifest and code-backed API.
    let (status, extensions) = request(&app, "GET", "/api/v1/admin/extensions", Some(&admin), None, None).await;
    assert_eq!(status, StatusCode::OK);
    let listed = extensions.as_array().unwrap();
    assert!(listed.iter().any(|ext| ext["id"] == "cloudflared"));

    // A node with a known bearer token (enrollment itself is covered elsewhere).
    let node_token = auth::random_token("wmn_").unwrap();
    let wg_key = STANDARD.encode([7u8; 32]);
    let node = db::create_node(
        &state.db,
        "mac-test",
        "mac-test.local",
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

    // Configure the named tunnel for the node. Credentials are write-only in
    // the API, but must be complete enough to pass validation.
    let tunnel_id = Uuid::new_v4();
    let credentials = json!({
        "AccountTag": "account",
        "TunnelSecret": "secret",
        "TunnelID": tunnel_id.to_string(),
        "Endpoint": ""
    });
    let (status, settings) = request(
        &app,
        "PUT",
        &format!("/api/v1/admin/extensions/cloudflared/nodes/{}", node.uuid()),
        Some(&admin),
        None,
        Some(json!({
            "binary": "cloudflared",
            "tunnel_id": tunnel_id.to_string(),
            "credentials_json": credentials.to_string()
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    assert_eq!(settings["has_credentials"], true);
    assert!(settings.get("credentials").is_none());
    assert_eq!(settings["tunnel_id"], tunnel_id.to_string());

    // Rotating the tunnel ID without rotating the secret is rejected before it
    // can reach the node.
    let other_tunnel = Uuid::new_v4();
    let (status, _) = request(
        &app,
        "PUT",
        &format!("/api/v1/admin/extensions/cloudflared/nodes/{}", node.uuid()),
        Some(&admin),
        None,
        Some(json!({"tunnel_id": other_tunnel.to_string()})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // One quick route and one named route.
    let (status, quick) = request(
        &app,
        "POST",
        "/api/v1/admin/extensions/cloudflared/routes",
        Some(&admin),
        None,
        Some(json!({
            "project": "site-quick",
            "node_id": node.uuid(),
            "local_url": "http://127.0.0.1:3000",
            "mode": "quick",
            "enabled": true
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{quick}");
    assert_eq!(quick["mode"], "quick");
    let quick_id = quick["id"].as_str().unwrap().to_string();

    let (status, named) = request(
        &app,
        "POST",
        "/api/v1/admin/extensions/cloudflared/routes",
        Some(&admin),
        None,
        Some(json!({
            "project": "site-named",
            "node_id": node.uuid(),
            "local_url": "http://localhost:4000",
            "mode": "named",
            "hostname": "app.example.com",
            "enabled": true
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{named}");
    let named_id = named["id"].as_str().unwrap().to_string();

    // A non-loopback target must be rejected: a node must not become a pivot.
    let (status, _) = request(
        &app,
        "POST",
        "/api/v1/admin/extensions/cloudflared/routes",
        Some(&admin),
        None,
        Some(json!({
            "project": "bad",
            "node_id": node.uuid(),
            "local_url": "http://10.66.0.99:3000",
            "mode": "quick",
            "enabled": true
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The node's map carries only its desired state, including the secret.
    let (status, map_value) = request(
        &app,
        "GET",
        "/api/v1/map?version=0&wait=1",
        None,
        Some(&node_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{map_value}");
    let map: NetworkMap = serde_json::from_value(map_value).unwrap();
    let cloudflared = map
        .extensions
        .iter()
        .find(|ext| ext.id == "cloudflared")
        .expect("cloudflared desired state in map");
    assert_eq!(cloudflared.driver, "cloudflared");
    assert!(cloudflared.enabled);
    assert_eq!(cloudflared.config["quick"][0]["project"], "site-quick");
    assert_eq!(cloudflared.config["named"][0]["hostname"], "app.example.com");
    assert_eq!(
        cloudflared.config["tunnel"]["credentials_json"],
        credentials.to_string()
    );

    // The agent reports live state; the admin route list merges it.
    let (status, _) = request(
        &app,
        "POST",
        "/api/v1/node/report",
        None,
        Some(&node_token),
        Some(json!({
            "peers": [],
            "nat_type": "unknown",
            "relay_connected": false,
            "extensions": [{
                "id": "cloudflared",
                "name": "Cloudflare Tunnel",
                "driver": "cloudflared",
                "state": "running",
                "message": "1/2 rotas ativas",
                "public_urls": ["https://random.trycloudflare.com"],
                "details": {
                    "routes": [
                        {"id": quick_id, "project": "site-quick", "state": "running",
                         "public_url": "https://random.trycloudflare.com"},
                        {"id": named_id, "project": "site-named", "state": "starting",
                         "message": "conectando"}
                    ]
                },
                "updated_at": "2026-11-12T23:00:00Z"
            }]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, routes) = request(
        &app,
        "GET",
        "/api/v1/admin/extensions/cloudflared/routes",
        Some(&admin),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let routes = routes.as_array().unwrap();
    let quick_view = routes.iter().find(|r| r["id"] == quick_id).unwrap();
    assert_eq!(
        quick_view["status"]["public_url"],
        "https://random.trycloudflare.com"
    );
    let named_view = routes.iter().find(|r| r["id"] == named_id).unwrap();
    assert_eq!(named_view["status"]["state"], "starting");
    assert_eq!(named_view["status"]["message"], "conectando");
}

#[tokio::test]
async fn generic_extension_config_requires_exec_validation() {
    let (_dir, state) = boot().await;
    let app = wm_control::api::router(state.clone());
    let admin = login(&app).await;

    // Drop a declarative extension manifest and reload it.
    let root = state.cfg.data_dir.join("extensions").join("hello");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("extension.toml"),
        "id = \"hello\"\nname = \"Hello\"\ndescription = \"teste\"\nnode_driver = \"exec\"\n",
    )
    .unwrap();
    let (status, reload) = request(
        &app,
        "POST",
        "/api/v1/admin/extensions/reload",
        Some(&admin),
        None,
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reload}");
    assert!(reload["manifests"]
        .as_array()
        .unwrap()
        .iter()
        .any(|ext| ext["id"] == "hello"));

    let node_token = auth::random_token("wmn_").unwrap();
    let node = db::create_node(
        &state.db,
        "mac-hello",
        "mac-hello.local",
        "linux",
        "x86_64",
        "0.1.0",
        &STANDARD.encode([8u8; 32]),
        &[],
        &Default::default(),
        NodeStatus::Active,
        "test",
        &auth::hash_token(&node_token),
        state.overlay,
    )
    .await
    .unwrap();

    let (status, _) = request(
        &app,
        "PUT",
        &format!("/api/v1/admin/extensions/hello/nodes/{}/config", node.uuid()),
        Some(&admin),
        None,
        Some(json!({"enabled": true, "config": {"command": ""}})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, saved) = request(
        &app,
        "PUT",
        &format!("/api/v1/admin/extensions/hello/nodes/{}/config", node.uuid()),
        Some(&admin),
        None,
        Some(json!({
            "enabled": true,
            "config": {
                "command": "hello-sidecar",
                "args": ["--port", "9000"],
                "public_urls": ["https://hello.example.com"]
            }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["enabled"], true);
    assert_eq!(saved["config"]["command"], "hello-sidecar");

    let (_, map_value) = request(
        &app,
        "GET",
        "/api/v1/map?version=0&wait=1",
        None,
        Some(&node_token),
        None,
    )
    .await;
    let map: NetworkMap = serde_json::from_value(map_value).unwrap();
    let hello = map
        .extensions
        .iter()
        .find(|ext| ext.id == "hello")
        .expect("hello desired state");
    assert_eq!(hello.driver, "exec");
    assert_eq!(hello.config["command"], "hello-sidecar");
}


#[tokio::test]
async fn node_token_can_toggle_extension_activation() {
    let (_dir, state) = boot().await;
    let app = wm_control::api::router(state.clone());
    let node_token = auth::random_token("wmn_").unwrap();
    let node = db::create_node(
        &state.db,
        "mac-toggle",
        "mac-toggle.local",
        "linux",
        "x86_64",
        "0.1.0",
        &STANDARD.encode([11u8; 32]),
        &[],
        &Default::default(),
        NodeStatus::Active,
        "test",
        &auth::hash_token(&node_token),
        state.overlay,
    )
    .await
    .unwrap();

    let (status, catalog) = request(
        &app,
        "GET",
        "/api/v1/node/extensions",
        None,
        Some(&node_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{catalog}");
    let cloudflared = catalog
        .as_array()
        .unwrap()
        .iter()
        .find(|view| view["id"] == "cloudflared")
        .unwrap();
    assert_eq!(cloudflared["active"], false);

    let (status, view) = request(
        &app,
        "PUT",
        "/api/v1/node/extensions/cloudflared",
        None,
        Some(&node_token),
        Some(json!({"active": true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["active"], true);

    let (_, map_value) = request(
        &app,
        "GET",
        "/api/v1/map?version=0&wait=1",
        None,
        Some(&node_token),
        None,
    )
    .await;
    let map: NetworkMap = serde_json::from_value(map_value).unwrap();
    assert!(map.extensions.iter().any(|desired| desired.id == "cloudflared"));
    assert_eq!(node.uuid().to_string().len(), 36);
}


#[tokio::test]
async fn admin_can_activate_an_extension_for_a_node() {
    let (_dir, state) = boot().await;
    let app = wm_control::api::router(state.clone());
    let admin = login(&app).await;
    let token = auth::random_token("wmn_").unwrap();
    let node = db::create_node(
        &state.db,
        "mac-admin-seance",
        "mac-admin-seance.local",
        "linux",
        "x86_64",
        "0.1.0",
        &STANDARD.encode([12u8; 32]),
        &[],
        &Default::default(),
        NodeStatus::Active,
        "test",
        &auth::hash_token(&token),
        state.overlay,
    )
    .await
    .unwrap();

    let (status, view) = request(
        &app,
        "PUT",
        &format!(
            "/api/v1/admin/extensions/seance/nodes/{}/activation",
            node.uuid()
        ),
        Some(&admin),
        None,
        Some(json!({"active": true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["node_id"], node.uuid().to_string());
    assert_eq!(view["active"], true);

    let (status, detail) = request(
        &app,
        "GET",
        "/api/v1/admin/extensions/seance",
        Some(&admin),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    let node_view = detail["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["node_id"] == node.uuid().to_string())
        .unwrap();
    assert_eq!(node_view["active"], true);
}
