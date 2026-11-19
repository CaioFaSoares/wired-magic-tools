use super::*;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;

fn fixture(data: &Path) -> String {
    let www = data.join("extensions/control_panel/www");
    fs::create_dir_all(www.join("assets")).unwrap();
    let mut files = BTreeMap::new();
    for (name, contents) in [("index.html", b"<!doctype html><title>optional panel</title>".as_slice()),
        ("assets/app-a1.js", b"console.log('panel');".as_slice()),
        ("assets/app-b2.css", b"body{}".as_slice()), ("favicon.svg", b"<svg/>".as_slice())] {
        fs::write(www.join(name), contents).unwrap();
        files.insert(name, format!("{:x}", Sha256::digest(contents)));
    }
    let manifest = serde_json::to_vec(&json!({"schema":1,"id":ID,"version":"test","files":files})).unwrap();
    fs::write(www.join("bundle.json"), &manifest).unwrap();
    format!("{:x}", Sha256::digest(&manifest))
}

fn enable(digest: &str) -> RuntimeInput {
    RuntimeInput { enabled: true, digest: Some(digest.to_string()) }
}

#[test]
fn installation_never_enables_but_confirmed_digest_enables_without_restart() {
    let data = tempfile::tempdir().unwrap();
    let runtime = PanelRuntime::load(data.path());
    assert!(!runtime.inspect().installed);
    let digest = fixture(data.path());
    let status = runtime.inspect();
    assert!(status.installed);
    assert!(!status.enabled);
    assert_eq!(status.digest.as_deref(), Some(digest.as_str()));
    assert!(runtime.set(RuntimeInput { enabled:true, digest:None }).is_err());
    assert!(runtime.set(enable(&"0".repeat(64))).is_err());
    assert!(runtime.set(enable(&digest)).unwrap().enabled);
    assert!(runtime.snapshot().unwrap().asset("nodes/example").is_some());
    assert!(PanelRuntime::load(data.path()).inspect().enabled);
    assert!(!runtime.set(RuntimeInput { enabled:false, digest:None }).unwrap().enabled);
    assert!(!PanelRuntime::load(data.path()).inspect().enabled);
}

#[test]
fn snapshot_is_immutable_and_modified_package_requires_reactivation() {
    let data = tempfile::tempdir().unwrap();
    let digest = fixture(data.path());
    let runtime = PanelRuntime::load(data.path());
    runtime.set(enable(&digest)).unwrap();
    let snapshot = runtime.snapshot().unwrap();
    fs::write(data.path().join("extensions/control_panel/www/assets/app-a1.js"), b"modified").unwrap();
    assert_eq!(snapshot.asset("assets/app-a1.js").unwrap().1.bytes.as_ref(), b"console.log('panel');");
    assert!(load_bundle(data.path()).is_err());
    assert!(!PanelRuntime::load(data.path()).inspect().enabled);
    // A failed refresh never replaces the previously trusted live snapshot.
    assert!(runtime.set(enable(&digest)).is_err());
    assert!(runtime.snapshot().is_some());
}

#[test]
fn only_safe_allowlisted_paths_and_actual_assets_are_served() {
    let data = tempfile::tempdir().unwrap();
    fixture(data.path());
    let bundle = load_bundle(data.path()).unwrap();
    for path in ["", "index.html", "login", "nodes/abc", "assets/app-a1.js", "favicon.svg"] {
        assert!(bundle.asset(path).is_some(), "{path}");
    }
    for path in ["../index.html", "/index.html", "assets/../index.html", "assets//x", ".env",
        "assets", "assets/missing", "assets/missing.js", "missing.css", "bundle.json", "extension.toml",
        "%2e%2e/secret", "assets%2fapp-a1.js", "assets\\app-a1.js", "nodes//abc", "foo?bar"] {
        assert!(bundle.asset(path).is_none(), "{path}");
    }
}

#[test]
fn invalid_manifests_paths_hashes_and_file_types_fail_closed() {
    let data = tempfile::tempdir().unwrap();
    fixture(data.path());
    let path = data.path().join("extensions/control_panel/www/bundle.json");
    let original: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for extra in [json!({"../secret.js":"0".repeat(64)}), json!({"x.html":"0".repeat(64)}),
        json!({"config.json":"0".repeat(64)}), json!({"assets/x.js":"not-a-digest"})] {
        let mut manifest = original.clone();
        manifest["files"].as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(load_bundle(data.path()).is_err());
    }
    for key in ["schema", "id", "files"] {
        let mut manifest = original.clone();
        manifest.as_object_mut().unwrap().remove(key);
        fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(load_bundle(data.path()).is_err());
    }
    let mut manifest = original;
    manifest["files"].as_object_mut().unwrap().remove("index.html");
    fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(load_bundle(data.path()).is_err());
}

#[test]
fn bounded_reads_reject_large_and_nonregular_files() {
    let data = tempfile::tempdir().unwrap();
    let path = data.path().join("large.js");
    let file = File::create(&path).unwrap();
    file.set_len(MAX_FILE_BYTES as u64 + 1).unwrap();
    assert!(read_bounded(File::open(&path).unwrap(), MAX_FILE_BYTES).is_err());
    assert!(read_bounded(File::open(data.path()).unwrap(), 10).is_err());
    assert!(read_bounded(File::open(&path).unwrap(), 0).is_err());
}

#[cfg(unix)]
#[test]
fn symlinks_in_every_package_component_and_trust_marker_are_rejected() {
    use std::os::unix::fs::symlink;
    for component in ["extensions", "extensions/control_panel", "extensions/control_panel/www",
        "extensions/control_panel/www/assets", "extensions/control_panel/www/assets/app-a1.js",
        "extensions/control_panel/www/bundle.json"] {
        let data = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        fixture(data.path());
        let source = data.path().join(component);
        let target = other.path().join("original");
        fs::rename(&source, &target).unwrap();
        symlink(&target, &source).unwrap();
        assert!(load_bundle(data.path()).is_err(), "{component}");
    }
    let data = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let digest = fixture(data.path());
    fs::write(other.path().join("trust"), &digest).unwrap();
    symlink(other.path().join("trust"), data.path().join("control-panel-trust.sha256")).unwrap();
    assert!(!PanelRuntime::load(data.path()).inspect().enabled);
}

async fn response(app: &Router, method: &str, path: &str, cookie: Option<&str>, csrf: Option<&str>, body: Value) -> axum::response::Response {
    let mut request = Request::builder().method(method).uri(path).header("content-type", "application/json");
    if let Some(cookie) = cookie { request = request.header("cookie", cookie); }
    if let Some(csrf) = csrf { request = request.header("x-wm-csrf", csrf); }
    app.clone().oneshot(request.body(Body::from(serde_json::to_vec(&body).unwrap())).unwrap()).await.unwrap()
}

#[tokio::test]
async fn http_runtime_requires_admin_csrf_digest_and_preserves_core_routes() {
    let data = tempfile::tempdir().unwrap();
    let mut cfg = crate::config::ControlConfig::default();
    cfg.data_dir = data.path().to_path_buf();
    cfg.admin_password = Some("panel-test-secret".into());
    let (state, _) = AppState::new(cfg).await.unwrap();
    let app = crate::api::router(state);
    let runtime = "/api/v1/extensions/control_panel/runtime";
    assert_eq!(response(&app, "GET", "/", None, None, Value::Null).await.status(), StatusCode::OK);
    assert_eq!(response(&app, "GET", "/login", None, None, Value::Null).await.status(), StatusCode::NOT_FOUND);
    assert_eq!(response(&app, "GET", runtime, None, None, Value::Null).await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(response(&app, "PUT", runtime, None, None, json!({"enabled":true})).await.status(), StatusCode::UNAUTHORIZED);
    let login = response(&app, "POST", "/api/v1/admin/login", None, None,
        json!({"username":"admin","password":"panel-test-secret"})).await;
    assert_eq!(login.status(), StatusCode::OK);
    let cookie = login.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    let value: Value = serde_json::from_slice(&to_bytes(login.into_body(), 4096).await.unwrap()).unwrap();
    let csrf = value["csrf_token"].as_str().unwrap();
    let digest = fixture(data.path());
    assert_eq!(response(&app, "PUT", runtime, Some(&cookie), None, json!({"enabled":true,"digest":digest})).await.status(), StatusCode::FORBIDDEN);
    assert_eq!(response(&app, "PUT", runtime, Some(&cookie), Some(csrf), json!({"enabled":true,"digest":"0".repeat(64)})).await.status(), StatusCode::BAD_REQUEST);
    assert_eq!(response(&app, "PUT", runtime, Some(&cookie), Some(csrf), json!({"enabled":true,"digest":digest})).await.status(), StatusCode::OK);
    let root = response(&app, "GET", "/", None, None, Value::Null).await;
    assert!(root.headers()["content-type"].to_str().unwrap().starts_with("text/html"));
    let deep = response(&app, "GET", "/nodes/example", None, None, Value::Null).await;
    assert_eq!(deep.status(), StatusCode::OK);
    assert_eq!(deep.headers()["cache-control"], "no-cache");
    let asset = response(&app, "GET", "/assets/app-a1.js", None, None, Value::Null).await;
    assert!(asset.headers()["content-type"].to_str().unwrap().starts_with("text/javascript"));
    assert_eq!(asset.headers()["x-content-type-options"], "nosniff");
    assert_eq!(asset.headers()["cross-origin-resource-policy"], "same-origin");
    let head = response(&app, "HEAD", "/assets/app-a1.js", None, None, Value::Null).await;
    assert_eq!(head.status(), StatusCode::OK);
    assert!(to_bytes(head.into_body(), 64).await.unwrap().is_empty());
    for path in ["/assets/missing.js", "/api/v1/missing", "/download/missing", "/j", "/relay/missing", "/assets/%2e%2e/secret"] {
        assert_eq!(response(&app, "GET", path, None, None, Value::Null).await.status(), StatusCode::NOT_FOUND, "{path}");
    }
    assert_eq!(response(&app, "GET", "/healthz", None, None, Value::Null).await.status(), StatusCode::OK);
    assert_eq!(response(&app, "GET", "/api/v1/admin/nodes", None, None, Value::Null).await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(response(&app, "PUT", runtime, Some(&cookie), Some(csrf), json!({"enabled":false})).await.status(), StatusCode::OK);
    assert_eq!(response(&app, "GET", "/assets/app-a1.js", None, None, Value::Null).await.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn uninitialized_and_database_errors_never_publish_a_trusted_snapshot() {
    let data = tempfile::tempdir().unwrap();
    let mut cfg = crate::config::ControlConfig::default();
    cfg.data_dir = data.path().to_path_buf();
    cfg.bootstrap_username = Some("setup".into());
    cfg.bootstrap_password = Some("temporary-setup-secret".into());
    let (state, _) = AppState::new(cfg).await.unwrap();
    assert!(db::setup_required(&state.db).await.unwrap());
    let digest = fixture(data.path());
    // Simulate a stale trust restored from a copied/previous control directory.
    state.extensions.panel.set(enable(&digest)).unwrap();
    let app = crate::api::router(state.clone());
    for path in ["/index.html", "/login", "/assets/app-a1.js"] {
        assert_eq!(response(&app, "GET", path, None, None, Value::Null).await.status(), StatusCode::NOT_FOUND);
    }
    state.db.close().await;
    assert_eq!(response(&app, "GET", "/index.html", None, None, Value::Null).await.status(), StatusCode::NOT_FOUND);
}
