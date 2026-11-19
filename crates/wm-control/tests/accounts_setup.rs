//! Dedicated setup/account regressions. Run serially on constrained machines:
//! CARGO_BUILD_JOBS=1 cargo test -p wm-control --test accounts_setup -- --test-threads=1
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use serde_json::{Value, json};
use tower::ServiceExt;
use wm_control::{api, auth, config::ControlConfig, db, state::AppState};

const BOOT_PASSWORD: &str = "temporary-bootstrap-secret";
const ADMIN_PASSWORD: &str = "permanent-admin-secret";
const USER_PASSWORD: &str = "permanent-user-secret";

fn config(dir: &tempfile::TempDir) -> ControlConfig {
    ControlConfig {
        data_dir: dir.path().to_path_buf(),
        public_url: "https://wm.example.test".into(),
        bootstrap_username: Some("bootstrap".into()),
        bootstrap_password: Some(BOOT_PASSWORD.into()),
        ..Default::default()
    }
}

async fn boot() -> (tempfile::TempDir, Arc<AppState>, Router) {
    let dir = tempfile::tempdir().unwrap();
    let (state, generated) = AppState::new(config(&dir)).await.unwrap();
    assert!(generated.is_none());
    let app = api::router(state.clone());
    (dir, state, app)
}

async fn call(app: &Router, method: Method, uri: &str, body: Option<Value>, cookie: Option<&str>, csrf: Option<&str>) -> (StatusCode, HeaderMap, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    if body.is_some() { req = req.header(header::CONTENT_TYPE, "application/json"); }
    if let Some(cookie) = cookie { req = req.header(header::COOKIE, cookie); }
    if let Some(csrf) = csrf { req = req.header("x-wm-csrf", csrf); }
    let response = app.clone().oneshot(req.body(Body::from(body.map(|v| v.to_string()).unwrap_or_default())).unwrap()).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, headers, body)
}

fn cookie(headers: &HeaderMap) -> String {
    headers.get(header::SET_COOKIE).unwrap().to_str().unwrap().split(';').next().unwrap().to_owned()
}

async fn setup_login(app: &Router) -> (String, String) {
    let (status, headers, body) = call(app, Method::POST, "/api/v1/setup/login",
        Some(json!({"username":"bootstrap", "password": BOOT_PASSWORD})), None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let raw = headers.get(header::SET_COOKIE).unwrap().to_str().unwrap();
    assert!(raw.contains("HttpOnly") && raw.contains("SameSite=Strict") && raw.contains("Secure"));
    assert!(raw.contains("Path=/api/v1/setup") && raw.contains("Max-Age=600"));
    (cookie(&headers), body["csrf_token"].as_str().unwrap().to_owned())
}

fn completion() -> Value {
    json!({
        "admin": {"username":"owner", "password": ADMIN_PASSWORD},
        "user": {"username":"member", "password": USER_PASSWORD}
    })
}

async fn complete(app: &Router, token: &str, csrf: &str) -> Value {
    let (status, headers, body) = call(app, Method::POST, "/api/v1/setup/complete", Some(completion()), Some(token), Some(csrf)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let raw = headers.get(header::SET_COOKIE).unwrap().to_str().unwrap();
    assert!(raw.starts_with("wm_setup=;") && raw.contains("Max-Age=0"));
    assert!(!raw.contains("wm_session="));
    body
}

#[tokio::test]
async fn setup_success_sessions_roles_logout_and_replay() {
    let (_dir, state, app) = boot().await;
    assert!(state.cfg.bootstrap_password.is_none() && state.cfg.admin_password.is_none());
    let (_, _, status) = call(&app, Method::GET, "/api/v1/setup", None, None, None).await;
    assert_eq!(status, json!({"required":true}));
    // The temporary credential is never a human account or permanent admin.
    for uri in ["/api/v1/auth/login", "/api/v1/admin/login"] {
        let (status, _, _) = call(&app, Method::POST, uri,
            Some(json!({"username":"bootstrap", "password":BOOT_PASSWORD})), None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let (setup, csrf) = setup_login(&app).await;
    let (other_setup, other_csrf) = setup_login(&app).await;
    let (status, _, _) = call(&app, Method::GET, "/api/v1/admin/session", None, Some(&setup), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let result = complete(&app, &setup, &csrf).await;
    assert_eq!(result["admin"]["role"], "admin");
    assert_eq!(result["user"]["role"], "user");
    assert_ne!(result["admin"]["id"], result["user"]["id"]);
    assert!(!db::setup_required(&state.db).await.unwrap());
    assert!(db::bootstrap(&state.db).await.unwrap().is_none());
    let cleared: (Option<String>, Option<String>, i64) = sqlx::query_as("SELECT username, password_hash, consumed FROM account_setup").fetch_one(&state.db).await.unwrap();
    assert_eq!(cleared, (None, None, 1));
    let (sessions,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM setup_sessions").fetch_one(&state.db).await.unwrap();
    assert_eq!(sessions, 0);
    for (cookie, csrf) in [(&setup, &csrf), (&other_setup, &other_csrf)] {
        let (status, _, _) = call(&app, Method::POST, "/api/v1/setup/complete", Some(completion()), Some(cookie), Some(csrf)).await;
        assert_eq!(status, StatusCode::CONFLICT);
    }
    let (status, _, _) = call(&app, Method::POST, "/api/v1/setup/login",
        Some(json!({"username":"bootstrap", "password":BOOT_PASSWORD})), None, None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    for (name, password, role) in [("owner", ADMIN_PASSWORD, "admin"), ("member", USER_PASSWORD, "user")] {
        let (status, headers, body) = call(&app, Method::POST, "/api/v1/auth/login",
            Some(json!({"username":name,"password":password})), None, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["user"]["role"], role);
        let cookie = cookie(&headers);
        let csrf = body["csrf_token"].as_str().unwrap();
        let (status, _, session) = call(&app, Method::GET, "/api/v1/auth/session", None, Some(&cookie), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(session, body);
        let (status, _, _) = call(&app, Method::POST, "/api/v1/auth/logout", None, Some(&cookie), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, headers, body) = call(&app, Method::POST, "/api/v1/auth/logout", None, Some(&cookie), Some(csrf)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({"ok":true}));
        assert!(headers.get(header::SET_COOKIE).unwrap().to_str().unwrap().contains("Max-Age=0"));
        let (status, _, _) = call(&app, Method::GET, "/api/v1/auth/session", None, Some(&cookie), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
async fn invalid_setup_credentials_csrf_names_password_and_role_injection() {
    let (_dir, state, app) = boot().await;
    let (status, _, _) = call(&app, Method::POST, "/api/v1/setup/login",
        Some(json!({"username":"bootstrap", "password":"wrong-password"})), None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (cookie, csrf) = setup_login(&app).await;
    for (token, csrf) in [(None, None), (Some(cookie.as_str()), None), (Some(cookie.as_str()), Some("wrong-csrf"))] {
        let (status, _, _) = call(&app, Method::POST, "/api/v1/setup/complete", Some(completion()), token, csrf).await;
        assert_eq!(status, if token.is_none() { StatusCode::UNAUTHORIZED } else { StatusCode::FORBIDDEN });
    }
    for invalid in [
        json!({"admin":{"username":"OWNER", "password":ADMIN_PASSWORD}, "user":{"username":"owner", "password":USER_PASSWORD}}),
        json!({"admin":{"username":"owner", "password":"short"}, "user":{"username":"member", "password":USER_PASSWORD}}),
        json!({"admin":{"username":"invalid name", "password":ADMIN_PASSWORD}, "user":{"username":"member", "password":USER_PASSWORD}}),
    ] {
        let (status, _, _) = call(&app, Method::POST, "/api/v1/setup/complete", Some(invalid), Some(&cookie), Some(&csrf)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    let mut injection = completion();
    injection["user"]["role"] = json!("admin");
    let (status, _, _) = call(&app, Method::POST, "/api/v1/setup/complete", Some(injection), Some(&cookie), Some(&csrf)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM admin_users").fetch_one(&state.db).await.unwrap();
    assert_eq!(count, 0);
    assert!(db::setup_required(&state.db).await.unwrap());
}

#[tokio::test]
async fn setup_session_expires_and_login_is_rate_limited() {
    let (_dir, state, app) = boot().await;
    let (cookie, csrf) = setup_login(&app).await;
    sqlx::query("UPDATE setup_sessions SET expires_at = 0").execute(&state.db).await.unwrap();
    let (status, _, _) = call(&app, Method::POST, "/api/v1/setup/complete", Some(completion()), Some(&cookie), Some(&csrf)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // Cheap invalid bounded input still consumes the shared login budget.
    for _ in 0..9 {
        let (status, _, _) = call(&app, Method::POST, "/api/v1/setup/login",
            Some(json!({"username":"bootstrap", "password":""})), None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let (status, _, _) = call(&app, Method::POST, "/api/v1/auth/login",
        Some(json!({"username":"bootstrap", "password":""})), None, None).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn ordinary_user_cannot_admin_extensions_sse_or_become_node() {
    let (_dir, _state, app) = boot().await;
    let (setup, csrf) = setup_login(&app).await;
    complete(&app, &setup, &csrf).await;
    let (status, headers, body) = call(&app, Method::POST, "/api/v1/auth/login",
        Some(json!({"username":"member", "password":USER_PASSWORD,"role":"admin"})), None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["user"]["role"], "user");
    let token = cookie(&headers);
    for uri in ["/api/v1/admin/session", "/api/v1/admin/me", "/api/v1/admin/overview", "/api/v1/admin/settings", "/api/v1/admin/extensions", "/api/v1/admin/extensions/chat_gui", "/api/v1/admin/events"] {
        // Never consume SSE body: a missing gate must fail fast, not hang test.
        let request = Request::builder().uri(uri).header(header::COOKIE, &token).body(Body::empty()).unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{uri}");
    }
    let (status, _, _) = call(&app, Method::GET, "/api/v1/node/extensions", None, Some(&token), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = call(&app, Method::POST, "/api/v1/admin/login",
        Some(json!({"username":"member", "password":USER_PASSWORD})), None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = call(&app, Method::POST, "/api/v1/admin/login",
        Some(json!({"username":"owner", "password":ADMIN_PASSWORD})), None, None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn database_completion_rolls_back_both_accounts_marker_and_sessions() {
    let (_dir, state, app) = boot().await;
    let (cookie, csrf) = setup_login(&app).await;
    let hash = auth::hash_token(cookie.split_once('=').unwrap().1);
    // Bypass request validation deliberately to inject failure on SECOND INSERT.
    let failed = db::complete_setup(&state.db, &hash, &csrf, "same", "test-admin-hash", "same", "test-user-hash").await;
    assert!(failed.is_err());
    assert!(db::setup_required(&state.db).await.unwrap());
    assert!(db::bootstrap(&state.db).await.unwrap().is_some());
    assert!(db::setup_session_by_hash(&state.db, &hash).await.unwrap().is_some());
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM admin_users").fetch_one(&state.db).await.unwrap();
    assert_eq!(count, 0);
    let result = db::complete_setup(&state.db, &hash, &csrf, "owner", "test-admin-hash", "member", "test-user-hash").await.unwrap();
    assert!(result.is_some());
}

#[tokio::test]
async fn database_concurrent_completion_has_exactly_one_winner_and_revokes_all_sessions() {
    let (_dir, state, app) = boot().await;
    let (cookie, csrf) = setup_login(&app).await;
    let (other, _) = setup_login(&app).await;
    let hash = auth::hash_token(cookie.split_once('=').unwrap().1);
    let (one, two) = tokio::join!(
        db::complete_setup(&state.db, &hash, &csrf, "owner1", "hash-a", "member1", "hash-u"),
        db::complete_setup(&state.db, &hash, &csrf, "owner2", "hash-a", "member2", "hash-u"),
    );
    assert_eq!(usize::from(one.unwrap().is_some()) + usize::from(two.unwrap().is_some()), 1);
    let (users,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM admin_users").fetch_one(&state.db).await.unwrap();
    assert_eq!(users, 2);
    assert!(db::setup_session_by_hash(&state.db, &auth::hash_token(other.split_once('=').unwrap().1)).await.unwrap().is_none());
    assert!(!db::create_setup_session(&state.db, "old-verified-hash", "new-session", "csrf", 600).await.unwrap());
}

#[tokio::test]
async fn pending_restart_keeps_seed_and_completed_restart_ignores_env_equivalent_configuration() {
    let (dir, state, app) = boot().await;
    let original = db::bootstrap(&state.db).await.unwrap().unwrap().password_hash;
    drop(app);
    state.shutdown().await;
    let mut changed = config(&dir);
    changed.bootstrap_username = Some("replacement".into());
    changed.bootstrap_password = None;
    changed.bootstrap_password_file = Some(dir.path().join("missing-secret-file"));
    let (state, _) = AppState::new(changed.clone()).await.unwrap();
    assert_eq!(db::bootstrap(&state.db).await.unwrap().unwrap().password_hash, original);
    let app = api::router(state.clone());
    let (cookie, csrf) = setup_login(&app).await;
    complete(&app, &cookie, &csrf).await;
    // Deleting accounts must never resurrect the one-time bootstrap authority.
    sqlx::query("DELETE FROM admin_users").execute(&state.db).await.unwrap();
    drop(app);
    state.shutdown().await;
    changed.admin_password = Some("ambiguous-but-ignored-after-consumed".into());
    let (state, _) = AppState::new(changed).await.unwrap();
    assert!(!db::setup_required(&state.db).await.unwrap());
    assert!(db::bootstrap(&state.db).await.unwrap().is_none());
    assert!(sqlx::query("UPDATE account_setup SET consumed = 0").execute(&state.db).await.is_err());
}

#[tokio::test]
async fn fresh_config_fails_closed_rejects_ambiguity_and_preserves_explicit_legacy() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = ControlConfig { data_dir: dir.path().to_path_buf(), ..Default::default() };
    let error = AppState::new(cfg.clone()).await.err().unwrap().to_string();
    assert!(error.contains("WM_BOOTSTRAP_USERNAME"));
    cfg.bootstrap_username = Some("bootstrap".into());
    cfg.bootstrap_password = Some(BOOT_PASSWORD.into());
    cfg.admin_password = Some("secret".into());
    assert!(AppState::new(cfg.clone()).await.err().unwrap().to_string().contains("ambígua"));
    cfg.bootstrap_username = None;
    cfg.bootstrap_password = None;
    let (state, generated) = AppState::new(cfg).await.unwrap();
    assert!(generated.is_none());
    assert!(!db::setup_required(&state.db).await.unwrap());
    let app = api::router(state);
    let (status, _, _) = call(&app, Method::POST, "/api/v1/admin/login", Some(json!({"username":"admin","password":"secret"})), None, None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn migrates_old_admin_schema_without_reopening_setup_or_replacing_password() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("control.db");
    let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1)
        .connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path).create_if_missing(true)).await.unwrap();
    sqlx::query("CREATE TABLE admin_users(id TEXT PRIMARY KEY, username TEXT NOT NULL UNIQUE, password_hash TEXT NOT NULL, created_at INTEGER NOT NULL)")
        .execute(&pool).await.unwrap();
    let id = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO admin_users VALUES(?, 'previous-admin', 'previous-password-hash', 0)")
        .bind(id.to_string()).execute(&pool).await.unwrap();
    pool.close().await;
    let mut cfg = config(&dir);
    cfg.bootstrap_password = None;
    cfg.bootstrap_password_file = Some(dir.path().join("missing-secret-file"));
    cfg.admin_password = Some("must-not-replace-password".into());
    let (state, _) = AppState::new(cfg).await.unwrap();
    let old = db::admin_by_username(&state.db, "previous-admin").await.unwrap().unwrap();
    assert_eq!(old.id, id.to_string());
    assert_eq!(old.role, "admin");
    assert_eq!(old.password_hash, "previous-password-hash");
    assert!(!db::setup_required(&state.db).await.unwrap());
    assert!(db::bootstrap(&state.db).await.unwrap().is_none());
}

#[tokio::test]
async fn sdk_bootstrap_account_and_panel_contract_roundtrip_over_http() {
    use wm_ext::admin::{AdminClient, AdminError};
    use wm_proto::{AccountCredentials, AccountRole, SetupCompleteRequest};

    let (_dir, _state, app) = boot().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = AdminClient::new(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
    assert!(client.setup_status().await.unwrap().required);
    let setup = client.setup_login("bootstrap", BOOT_PASSWORD).await.unwrap();
    let request = SetupCompleteRequest {
        admin: AccountCredentials { username: "owner".into(), password: ADMIN_PASSWORD.into() },
        user: AccountCredentials { username: "member".into(), password: USER_PASSWORD.into() },
    };
    let created = client.complete_setup(&setup, &request).await.unwrap();
    assert_eq!(created.admin.role, AccountRole::Admin);
    assert_eq!(created.user.role, AccountRole::User);
    assert!(!client.setup_status().await.unwrap().required);
    assert!(matches!(client.complete_setup(&setup, &request).await, Err(AdminError::Http { status: 409 })));
    let user = client.account_login("member", USER_PASSWORD).await.unwrap();
    assert_eq!(client.account_session(&user).await.unwrap().user.id, created.user.id);
    assert!(user.as_admin().is_err());
    client.account_logout(&user).await.unwrap();
    assert!(matches!(client.account_session(&user).await, Err(AdminError::Http { status: 401 })));
    let owner = client.account_login("owner", ADMIN_PASSWORD).await.unwrap();
    let admin = owner.as_admin().unwrap();
    assert_eq!(client.control_panel(&admin).await.unwrap()["enabled"], false);
    assert_eq!(client.set_control_panel(&admin, false, None).await.unwrap()["enabled"], false);
    client.account_logout(&owner).await.unwrap();
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}

#[cfg(unix)]
#[tokio::test]
async fn bootstrap_password_file_supports_secret_volume_symlinks_and_rejects_fifo() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    std::fs::write(&secret, format!("{BOOT_PASSWORD}\r\n")).unwrap();
    let link = dir.path().join("secret-link");
    symlink(&secret, &link).unwrap();
    let mut cfg = config(&dir);
    cfg.bootstrap_password = None;
    cfg.bootstrap_password_file = Some(link);
    let (state, _) = AppState::new(cfg).await.unwrap();
    assert!(state.cfg.bootstrap_password_file.is_none());
    setup_login(&api::router(state.clone())).await;
    state.shutdown().await;

    let fresh = tempfile::tempdir().unwrap();
    let fifo = fresh.path().join("secret-fifo");
    let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: name is a valid NUL-terminated path inside this owned temporary directory.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let mut cfg = config(&fresh);
    cfg.bootstrap_password = None;
    cfg.bootstrap_password_file = Some(fifo);
    let error = AppState::new(cfg).await.err().unwrap().to_string();
    assert!(error.contains("regular"));
}

#[test]
fn secret_debug_is_redacted_and_contract_serializes_roles() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(&dir);
    assert!(!format!("{cfg:?}").contains(BOOT_PASSWORD));
    let request = wm_proto::LoginRequest { username: "bootstrap".into(), password: BOOT_PASSWORD.into() };
    assert!(!format!("{request:?}").contains(BOOT_PASSWORD));
    assert_eq!(serde_json::to_value(wm_proto::AccountRole::Admin).unwrap(), json!("admin"));
    assert_eq!(serde_json::to_value(wm_proto::AccountRole::User).unwrap(), json!("user"));
}
