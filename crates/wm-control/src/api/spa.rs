//! Headless by default. Optional UI is an explicitly trusted server extension.
use std::sync::Arc;

use axum::extract::State;
use axum::http::{Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

use crate::db;
use crate::extensions::control_panel::PanelBundle;
use crate::state::AppState;

pub async fn handler(State(state): State<Arc<AppState>>, method: Method, uri: Uri) -> Response {
    if method != Method::GET && method != Method::HEAD {
        let mut response = (StatusCode::METHOD_NOT_ALLOWED, "método não permitido").into_response();
        response.headers_mut().insert(header::ALLOW, "GET, HEAD".parse().unwrap());
        return secure(response, "no-store");
    }
    if reserved(uri.path()) {
        return not_found();
    }
    let Some(bundle) = state.extensions.panel.snapshot() else {
        return headless(uri.path());
    };
    // Bootstrapping must never publish login/UI, even if a stale trust marker
    // exists. Errors fail closed. This does not change authentication of APIs.
    if !matches!(db::setup_required(&state.db).await, Ok(false)) {
        return headless(uri.path());
    }
    serve(&bundle, &method, uri.path())
}

fn reserved(path: &str) -> bool {
    ["/api", "/relay", "/healthz", "/j", "/download"].iter()
        .any(|prefix| path == *prefix || path.strip_prefix(prefix).is_some_and(|tail| tail.starts_with('/')))
}

fn headless(path: &str) -> Response {
    if path != "/" { return not_found(); }
    secure((StatusCode::OK, "wired-magic control: headless\nUse a CLI/TUI administrativa. Painel React opcional desativado.\n").into_response(), "no-store")
}

fn serve(bundle: &PanelBundle, method: &Method, path: &str) -> Response {
    if reserved(path) { return not_found(); }
    let Some(requested) = path.strip_prefix('/') else { return not_found(); };
    let Some((name, asset)) = bundle.asset(requested) else { return not_found(); };
    let length = asset.bytes.len();
    let mut response = if method == Method::HEAD {
        ().into_response()
    } else {
        asset.bytes.into_response()
    };
    response.headers_mut().insert(header::CONTENT_TYPE, asset.mime.parse().unwrap());
    response.headers_mut().insert(header::CONTENT_LENGTH, length.to_string().parse().unwrap());
    secure(response, if name.starts_with("assets/") { "public, max-age=31536000, immutable" } else { "no-cache" })
}

fn not_found() -> Response {
    secure((StatusCode::NOT_FOUND, axum::Json(serde_json::json!({
        "error": { "code": "not_found", "message": "rota não encontrada" }
    }))).into_response(), "no-store")
}

fn secure(mut response: Response, cache: &'static str) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, cache.parse().unwrap());
    headers.insert("x-content-type-options", "nosniff".parse().unwrap());
    headers.insert("x-frame-options", "DENY".parse().unwrap());
    headers.insert("referrer-policy", "no-referrer".parse().unwrap());
    headers.insert("cross-origin-resource-policy", "same-origin".parse().unwrap());
    headers.insert("content-security-policy", "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self'; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'".parse().unwrap());
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_namespaces_never_become_spa_routes() {
        for path in ["/api", "/api/v1/missing", "/relay", "/relay/missing", "/healthz", "/j/missing", "/download/wm", "/download/missing"] {
            assert!(reserved(path), "{path}");
        }
        for path in ["/", "/login", "/nodes/example", "/apiary"] {
            assert!(!reserved(path), "{path}");
        }
    }

    #[test]
    fn default_root_is_headless_and_other_routes_are_not_found() {
        let root = headless("/");
        assert_eq!(root.status(), StatusCode::OK);
        assert_eq!(root.headers()[header::CACHE_CONTROL], "no-store");
        assert!(root.headers()[header::CONTENT_TYPE].to_str().unwrap().starts_with("text/plain"));
        assert_eq!(root.headers()["x-content-type-options"], "nosniff");
        for path in ["/login", "/assets/a.js", "/index.html"] {
            assert_eq!(headless(path).status(), StatusCode::NOT_FOUND);
        }
    }
}
