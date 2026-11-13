use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use tower::ServiceExt;

use wm_control::api;
use wm_control::config::ControlConfig;
use wm_control::state::AppState;

async fn boot() -> (Arc<AppState>, Router) {
    let dir = tempfile::tempdir().unwrap();
    // Keep the temp dir alive for the whole process by leaking it in the test.
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
        serde_json::from_slice(&bytes).unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into()))
    };
    (status, headers, value)
}

fn wg_key(name: &str) -> String {
    let seed = name
        .as_bytes()
        .iter()
        .fold(13u8, |acc, b| acc.wrapping_mul(31).wrapping_add(*b));
    STANDARD.encode([seed; 32])
}

async fn enroll(app: &Router, token: &str, name: &str) -> String {
    let (status, _, body) = call(
        app,
        Method::POST,
        "/api/v1/enroll",
        Some(json!({
            "join_token": token,
            "name": name,
            "hostname": format!("{name}.local"),
            "os": "linux",
            "arch": "aarch64",
            "version": "0.1.0",
            "wg_public_key": wg_key(name),
            "labels": {"baguette_port": "8421"},
            "tags": ["mac", "site"],
            "nat_type": "unknown"
        })),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["node_token"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn admin_enroll_map_acl_and_integration() {
    let (_state, app) = boot().await;

    let (status, _, health) = call(&app, Method::GET, "/healthz", None, None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(health["ok"], true);

    let (status, headers, login) = call(
        &app,
        Method::POST,
        "/api/v1/admin/login",
        Some(json!({"username":"admin","password":"secret"})),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{login}");
    let csrf = login["csrf_token"].as_str().unwrap().to_string();
    let cookie = headers
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    let (status, _, token) = call(
        &app,
        Method::POST,
        "/api/v1/admin/join-tokens",
        Some(json!({"name":"e2e","ttl_secs":3600,"max_uses":5,"tags":["mac"]})), 
        Some(&cookie),
        Some(&csrf),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{token}");
    let join_token = token["token"].as_str().unwrap().to_string();

    let token_a = enroll(&app, &join_token, "mac-a").await;
    let _token_b = enroll(&app, &join_token, "mac-b").await;

    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/map?version=0&wait=1")
        .header(header::AUTHORIZATION, format!("Bearer {token_a}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let map: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(map["me"]["name"], "mac-a");
    assert_eq!(map["peers"].as_array().unwrap().len(), 1);
    assert_eq!(map["peers"][0]["name"], "mac-b");
    // Client-supplied privileged tags must not escape the invitation's authority.
    assert_eq!(map["peers"][0]["tags"], json!(["mac"]));
    assert!(map["relay"]["token"].is_string());
    assert_eq!(map["acl"]["default_allow"], true);

    let yaml = "version: 1\nrules:\n  - action: accept\n    src: [\"tag:mac\"]\n    dst: [\"tag:mac\"]\n    protocol: tcp\n    ports: [\"8421\"]\n";
    let (status, _, acl) = call(
        &app,
        Method::PUT,
        "/api/v1/admin/acl",
        Some(json!({"yaml": yaml})),
        Some(&cookie),
        Some(&csrf),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{acl}");
    assert_eq!(acl["compiled"]["default_allow"], false);
    assert_eq!(acl["compiled"]["rules"].as_array().unwrap().len(), 4);

    let (status, _, sim) = call(
        &app,
        Method::POST,
        "/api/v1/admin/acl/simulate",
        Some(json!({"src":"mac-a","dst":"mac-b","protocol":"tcp","port":8421})),
        Some(&cookie),
        Some(&csrf),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(sim["allowed"], true);

    let (status, _, service) = call(
        &app,
        Method::POST,
        "/api/v1/admin/service-tokens",
        Some(json!({"name":"seance"})),
        Some(&cookie),
        Some(&csrf),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{service}");
    let service_token = service["token"].as_str().unwrap();

    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/integration/nodes")
        .header(header::AUTHORIZATION, format!("Bearer {service_token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(body["nodes"][0]["labels"]["baguette_port"], "8421");

    // Enrolling into an existing tag must update the persisted compiled ACL.
    let _token_c = enroll(&app, &join_token, "mac-c").await;
    let (_, _, acl) = call(
        &app, Method::GET, "/api/v1/admin/acl", None,
        Some(&cookie), None,
    ).await;
    assert_eq!(acl["compiled"]["rules"].as_array().unwrap().len(), 9);
    assert_eq!(acl["compiled"]["default_allow"], false);
}

#[tokio::test]
async fn forwarding_headers_cannot_bypass_login_rate_limit() {
    let (_state, app) = boot().await;
    for attempt in 0..11 {
        let req = Request::builder().method(Method::POST).uri("/api/v1/admin/login")
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-forwarded-for", format!("198.51.100.{attempt}"))
            .header("x-real-ip", format!("198.51.100.{attempt}"))
            .header("x-wm-internal-socket-ip", format!("198.51.100.{attempt}"))
            .extension(axum::extract::ConnectInfo("203.0.113.7:1234".parse::<std::net::SocketAddr>().unwrap()))
            .body(Body::from(r#"{"username":"missing","password":"wrong"}"#)).unwrap();
        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), if attempt < 10 { StatusCode::UNAUTHORIZED } else { StatusCode::TOO_MANY_REQUESTS });
    }
    let req = Request::builder().method(Method::POST).uri("/api/v1/admin/login")
        .header(header::CONTENT_TYPE, "application/json")
        .extension(axum::extract::ConnectInfo("203.0.113.8:1234".parse::<std::net::SocketAddr>().unwrap()))
        .body(Body::from(r#"{"username":"missing","password":"wrong"}"#)).unwrap();
    assert_eq!(app.oneshot(req).await.unwrap().status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn rejects_bad_admin_and_node_tokens() {
    let (_state, app) = boot().await;
    let (status, _, _) = call(
        &app,
        Method::POST,
        "/api/v1/admin/login",
        Some(json!({"username":"admin","password":"wrong"})),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/map")
        .header(header::AUTHORIZATION, "Bearer wmn_nope")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/integration/nodes")
        .header(header::AUTHORIZATION, "Bearer wms_nope")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
