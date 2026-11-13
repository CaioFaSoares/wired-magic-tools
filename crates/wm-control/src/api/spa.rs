use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../web/dist"]
struct Assets;

pub async fn handler(uri: Uri) -> Response {
    let path = uri.path();
    if path == "/api" || path.starts_with("/api/") || path == "/relay" || path == "/healthz" {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({
                "error": { "code": "not_found", "message": "rota não encontrada" }
            })),
        )
            .into_response();
    }

    let requested = path.trim_start_matches('/');
    let mut name = if requested.is_empty() {
        "index.html".to_string()
    } else {
        requested.to_string()
    };

    let asset = Assets::get(&name).or_else(|| {
        // Missing static files must not be answered with the SPA document.
        if requested == "assets" || requested.starts_with("assets/")
            || requested.rsplit('/').next().is_some_and(|part| part.contains('.')) {
            return None;
        }
        name = "index.html".to_string();
        Assets::get(&name)
    });
    let Some(asset) = asset else {
        return (StatusCode::NOT_FOUND, "painel web não embutido").into_response();
    };
    let mime = mime_guess::from_path(&name)
        .first_or_octet_stream()
        .to_string();
    let cache = if name == "index.html" {
        "no-cache"
    } else if name.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, cache.to_string()),
        ],
        asset.data.to_vec(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn spa_routes_are_html_and_not_immutable() {
        for path in ["/", "/login", "/nodes", "/nodes/example", "/tokens", "/acl"] {
            let response = handler(path.parse().unwrap()).await;
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert_eq!(response.headers()[header::CONTENT_TYPE], "text/html");
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
        }
    }

    #[tokio::test]
    async fn embedded_javascript_has_asset_mime_and_cache() {
        let name = Assets::iter().find(|name| name.starts_with("assets/") && name.ends_with(".js"))
            .expect("frontend bundle must contain JavaScript");
        let response = handler(format!("/{name}").parse().unwrap()).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], mime_guess::from_path(name.as_ref()).first_or_octet_stream().as_ref());
        assert_eq!(response.headers()[header::CACHE_CONTROL], "public, max-age=31536000, immutable");
    }

    #[tokio::test]
    async fn missing_assets_and_api_do_not_fall_back() {
        for path in ["/assets/missing.js", "/assets/missing", "/missing.css", "/missing.ico", "/api", "/api/missing", "/relay", "/healthz"] {
            assert_eq!(handler(path.parse().unwrap()).await.status(), StatusCode::NOT_FOUND, "{path}");
        }
    }
}
