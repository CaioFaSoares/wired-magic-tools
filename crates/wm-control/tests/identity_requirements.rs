//! Identity (display profile) and per-node requirements over the HTTP API.
//!
//! These tests exercise the contract the TUI depends on: the technical name
//! stays the ACL identity while `display_name`/`about` carry presentation, and
//! the server evaluates requirements from *observed* reports.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use axum::Router;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use tower::ServiceExt;

use wm_control::api;
use wm_control::config::ControlConfig;
use wm_control::state::AppState;

async fn boot() -> (Arc<AppState>, Router) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.keep();
    let mut cfg = ControlConfig::default();
    cfg.data_dir = path;
    cfg.public_url = "http://127.0.0.1:8080".into();
    cfg.admin_password = Some("secret".into());
    let (state, generated) = AppState::new(cfg).await.unwrap();
    assert!(generated.is_none());
    let router = api::router(state.clone());
    (state, router)
}

async fn call(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
    cookie: Option<&str>,
    csrf: Option<&str>,
) -> (StatusCode, HeaderMap, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    if let Some(csrf) = csrf {
        builder = builder.header("x-wm-csrf", csrf);
    }
    let req = builder
        .body(Body::from(body.map(|b| b.to_string()).unwrap_or_default()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into()))
    };
    (status, headers, value)
}

async fn call_bearer(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
    token: &str,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    let req = builder
        .body(Body::from(body.map(|b| b.to_string()).unwrap_or_default()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into()))
    };
    (status, value)
}

fn wg_key(name: &str) -> String {
    let seed = name
        .as_bytes()
        .iter()
        .fold(17u8, |acc, b| acc.wrapping_mul(31).wrapping_add(*b));
    STANDARD.encode([seed; 32])
}

async fn login(app: &Router) -> (String, String) {
    let (status, headers, body) = call(
        app,
        Method::POST,
        "/api/v1/admin/login",
        Some(json!({"username":"admin","password":"secret"})),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let csrf = body["csrf_token"].as_str().unwrap().to_string();
    let cookie = headers
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    (cookie, csrf)
}

async fn create_invite(
    app: &Router,
    cookie: &str,
    csrf: &str,
    name: &str,
    require_approval: bool,
) -> String {
    let (status, _, body) = call(
        app,
        Method::POST,
        "/api/v1/admin/join-tokens",
        Some(json!({
            "name": name,
            "ttl_secs": 3600,
            "max_uses": 4,
            "tags": ["mac"],
            "require_approval": require_approval,
        })),
        Some(cookie),
        Some(csrf),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["require_approval"], require_approval);
    body["token"].as_str().unwrap().to_string()
}

async fn enroll(
    app: &Router,
    join_token: &str,
    name: &str,
    display_name: &str,
    about: &str,
) -> (String, Value) {
    let (status, _, body) = call(
        app,
        Method::POST,
        "/api/v1/enroll",
        Some(json!({
            "join_token": join_token,
            "name": name,
            "hostname": format!("{name}.local"),
            "os": "linux",
            "arch": "aarch64",
            "version": "0.1.0",
            "wg_public_key": wg_key(name),
            "display_name": display_name,
            "about": about,
        })),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    (body["node_token"].as_str().unwrap().to_string(), body)
}

#[tokio::test]
async fn display_profile_is_presentation_only_and_editable_by_the_node() {
    let (_state, app) = boot().await;
    let (cookie, csrf) = login(&app).await;
    let invite = create_invite(&app, &cookie, &csrf, "identity", false).await;
    let (node_token, enrolled) = enroll(&app, &invite, "amigos", "  Caio  ", "rack da sala").await;
    assert_eq!(enrolled["node_name"], "amigos");

    let (status, map) = call_bearer(&app, Method::GET, "/api/v1/map?version=0&wait=1", None, &node_token).await;
    assert_eq!(status, StatusCode::OK, "{map}");
    // It is sent to others exactly like any other presentation field.
    assert_eq!(map["me"]["name"], "amigos");
    assert_eq!(map["me"]["display_name"], "Caio");
    assert_eq!(map["me"]["about"], "rack da sala");

    let (status, updated) = call_bearer(
        &app,
        Method::PATCH,
        "/api/v1/node/profile",
        Some(json!({"display_name":"Caio (amigos)", "about":"  sala ao lado  "})),
        &node_token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["display_name"], "Caio (amigos)");
    assert_eq!(updated["about"], "sala ao lado");
    // The technical name is untouched and remains the ACL identity.
    assert_eq!(updated["name"], "amigos");

    let (status, rejected) = call_bearer(
        &app,
        Method::PATCH,
        "/api/v1/node/profile",
        Some(json!({"display_name":"quebrado\u{0007}", "about":""})),
        &node_token,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{rejected}");
    assert_eq!(rejected["error"]["code"], "invalid_profile");
}

#[tokio::test]
async fn requirements_are_evaluated_from_reports_and_never_kick_the_node() {
    let (_state, app) = boot().await;
    let (cookie, csrf) = login(&app).await;
    let invite = create_invite(&app, &cookie, &csrf, "requirements", false).await;
    let (node_token, _) = enroll(&app, &invite, "tecnico", "", "").await;

    let (status, _, nodes) = call(&app, Method::GET, "/api/v1/admin/nodes", None, Some(&cookie), None).await;
    assert_eq!(status, StatusCode::OK);
    let node_id = nodes[0]["id"].as_str().unwrap().to_string();

    let (status, _, view) = call(
        &app,
        Method::PUT,
        &format!("/api/v1/admin/nodes/{node_id}/requirements"),
        Some(json!({
            "min_version": "0.2.0",
            "required_extensions": ["cloudflared"],
            "required_tags": ["mac"],
            "agent": {"mtu": 1400, "force_relay": false},
        })),
        Some(&cookie),
        Some(&csrf),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["requirements"]["min_version"], "0.2.0");
    // No report yet: the stored runtime is empty, so requirements fail closed.
    assert_eq!(view["compliance"]["compliant"], false);

    let report = |version: &str, mtu: u16, state: &str| {
        json!({
            "peers": [],
            "nat_type": "unknown",
            "relay_connected": false,
            "version": version,
            "agent": {
                "udp_port": 41641,
                "mtu": mtu,
                "force_relay": false,
                "expose": [],
                "tun_name": "utun9",
            },
            "extensions": [{
                "id": "cloudflared",
                "name": "cloudflared",
                "driver": "sidecar",
                "state": state,
                "public_urls": [],
                "details": null,
                "updated_at": "2026-01-01T00:00:00Z",
            }],
        })
    };
    let (status, body) = call_bearer(
        &app,
        Method::POST,
        "/api/v1/node/report",
        Some(report("0.1.0", 1280, "stopped")),
        &node_token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (_, _, nodes) = call(&app, Method::GET, "/api/v1/admin/nodes", None, Some(&cookie), None).await;
    let compliance = &nodes[0]["compliance"];
    assert_eq!(compliance["compliant"], false);
    let failed: Vec<&str> = compliance["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|check| check["ok"] == false)
        .map(|check| check["id"].as_str().unwrap())
        .collect();
    assert!(failed.contains(&"min_version"));
    assert!(failed.contains(&"extension:cloudflared"));
    assert!(failed.contains(&"agent.mtu"));
    // Observation only: the node stays active.
    assert_eq!(nodes[0]["status"], "active");

    let (status, _) = call_bearer(
        &app,
        Method::POST,
        "/api/v1/node/report",
        Some(report("0.2.0", 1400, "running")),
        &node_token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, _, nodes) = call(&app, Method::GET, "/api/v1/admin/nodes", None, Some(&cookie), None).await;
    assert_eq!(nodes[0]["compliance"]["compliant"], true, "{}", nodes[0]);
}

#[tokio::test]
async fn empty_requirement_fields_clear_instead_of_requiring_empty() {
    let (_state, app) = boot().await;
    let (cookie, csrf) = login(&app).await;
    let invite = create_invite(&app, &cookie, &csrf, "clear", false).await;
    let (_token, _) = enroll(&app, &invite, "limpo", "", "").await;
    let (_, _, nodes) = call(&app, Method::GET, "/api/v1/admin/nodes", None, Some(&cookie), None).await;
    let node_id = nodes[0]["id"].as_str().unwrap().to_string();
    let (status, _, view) = call(
        &app,
        Method::PUT,
        &format!("/api/v1/admin/nodes/{node_id}/requirements"),
        Some(json!({
            "min_version": "   ",
            "required_extensions": [],
            "required_tags": [],
            "agent": {"expose": [], "tun_name": ""},
        })),
        Some(&cookie),
        Some(&csrf),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert!(view["requirements"]["min_version"].is_null());
    assert!(view["requirements"]["agent"]["expose"].is_null());
    assert!(view["requirements"]["agent"]["tun_name"].is_null());
    assert_eq!(view["compliance"]["checks"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn invitation_can_require_approval_while_default_stays_invite_only() {
    let (_state, app) = boot().await;
    let (cookie, csrf) = login(&app).await;

    let open = create_invite(&app, &cookie, &csrf, "open", false).await;
    let (_, body) = enroll(&app, &open, "aberto", "", "").await;
    assert_eq!(body["node_name"], "aberto");

    let gated = create_invite(&app, &cookie, &csrf, "gated", true).await;
    let (_token, _) = enroll(&app, &gated, "fechado", "", "").await;
    let (_, _, nodes) = call(&app, Method::GET, "/api/v1/admin/nodes", None, Some(&cookie), None).await;
    let gated_node = nodes
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["name"] == "fechado")
        .unwrap();
    assert_eq!(gated_node["status"], "pending");
    let id = gated_node["id"].as_str().unwrap().to_string();
    let (status, _, approved) = call(
        &app,
        Method::POST,
        &format!("/api/v1/admin/nodes/{id}/approve"),
        None,
        Some(&cookie),
        Some(&csrf),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    assert_eq!(approved["status"], "active");
}
