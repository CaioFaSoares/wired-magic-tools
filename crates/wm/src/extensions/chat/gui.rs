//! HTTP API and embedded web assets for chat.
use super::core::*;
use std::net::SocketAddr;
use anyhow::Context;
use axum::extract::{DefaultBodyLimit, Query, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{Value, json};

const INDEX_HTML: &str = include_str!("../../chat_assets/index.html");
const APP_CSS: &str = include_str!("../../chat_assets/app.css");
const APP_JS: &str = include_str!("../../chat_assets/app.js");
const MAX_BODY_BYTES: usize = 64 * 1024;
type ApiError = (StatusCode, Json<Value>);

fn bad_request(message: impl Into<String>) -> ApiError {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": message.into() })),
    )
}
async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; img-src 'self' data:; style-src 'self'; script-src 'self'; \
             connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
        ),
    );
    response
}

fn asset(body: &'static str, content_type: &'static str) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
        ],
        body,
    )
        .into_response()
}

async fn index() -> Response {
    asset(INDEX_HTML, "text/html; charset=utf-8")
}

async fn css() -> Response {
    asset(APP_CSS, "text/css; charset=utf-8")
}

async fn js() -> Response {
    asset(APP_JS, "text/javascript; charset=utf-8")
}

async fn health() -> Json<Value> {
    Json(json!({ "ok": true, "service": "wm-chat" }))
}

async fn info(State(state): State<ChatState>) -> Json<InfoResponse> { Json(state.room().info()) }
async fn list_messages(State(state): State<ChatState>, Query(query): Query<ListQuery>) -> Json<MessagesResponse> { Json(state.room().list(query.after)) }
async fn create_message(State(state): State<ChatState>, Json(input): Json<PostInput>) -> Result<(StatusCode, Json<PostResponse>), ApiError> { state.room().send(input).map(|response| (StatusCode::CREATED, Json(response))).map_err(bad_request) }
async fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "error": "rota não encontrada" })),
    )
        .into_response()
}

fn app(state: ChatState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/index.html", get(index))
        .route("/app.css", get(css))
        .route("/app.js", get(js))
        .route("/healthz", get(health))
        .route("/api/info", get(info))
        .route("/api/messages", get(list_messages).post(create_message))
        .fallback(not_found)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

/// Binds the chat server and serves until the process is stopped.
///
/// The manifest starts this with `--listen 127.0.0.1:8790`; the adapter then
/// exposes that loopback port on the overlay and publishes `chat-<node>`.
pub async fn run(listen: SocketAddr, title: String, node: Option<String>) -> anyhow::Result<()> {
    let state = ChatState::new(title, node);
    let app = app(state);
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("não foi possível escutar em {listen}"))?;
    println!("chat_gui: ouvindo em http://{listen}");
    axum::serve(listener, app)
        .await
        .context("servidor de chat encerrou com erro")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use serde_json::Value as JsonValue;
    use tower::ServiceExt;

    fn test_state() -> ChatState {
        ChatState::new("Sala de teste".into(), Some("nó-1".into()))
    }

    async fn body_json(response: Response) -> JsonValue {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn malformed_and_oversized_requests_do_not_mutate_room() {
        let state = test_state();
        let app = app(state.clone());
        for (body, expected) in [
            (vec![0xff], StatusCode::BAD_REQUEST),
            (br#"{"text":123}"#.to_vec(), StatusCode::UNPROCESSABLE_ENTITY),
            (json!({"text": "x".repeat(MAX_BODY_BYTES)}).to_string().into_bytes(), StatusCode::PAYLOAD_TOO_LARGE),
            (json!({"text": "🦀".repeat(MAX_TEXT_CHARS + 1)}).to_string().into_bytes(), StatusCode::BAD_REQUEST),
        ] {
            let response = app.clone().oneshot(Request::builder()
                .method("POST").uri("/api/messages")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body)).unwrap()).await.unwrap();
            assert_eq!(response.status(), expected);
        }
        assert_eq!(state.room().info().messages, 0);
    }

    #[tokio::test]
    async fn http_roundtrip_posts_and_lists_messages() {
        let app = app(test_state());

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/info")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let info = body_json(response).await;
        assert_eq!(info["title"], "Sala de teste");
        assert_eq!(info["node"], "nó-1");
        assert_eq!(info["max_text"], MAX_TEXT_CHARS);
        let epoch = info["epoch"].as_str().unwrap().to_string();

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/messages")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({ "author": "Ana", "text": "olá" }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let posted = body_json(response).await;
        assert_eq!(posted["epoch"], epoch);
        assert_eq!(posted["message"]["author"], "Ana");
        assert_eq!(posted["message"]["text"], "olá");
        assert_eq!(posted["message"]["id"], 1);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/messages?after=0")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let listed = body_json(response).await;
        assert_eq!(listed["messages"].as_array().unwrap().len(), 1);
        assert_eq!(listed["cursor"], 1);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/messages?after=1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let empty = body_json(response).await;
        assert!(empty["messages"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn rejects_empty_messages_and_sets_security_headers() {
        let app = app(test_state());

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/messages")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({ "author": "Ana", "text": "   " }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = body_json(response).await;
        assert!(body["error"].as_str().unwrap().contains("vazia"));

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        assert_eq!(
            response.headers()[header::X_CONTENT_TYPE_OPTIONS],
            "nosniff"
        );
        assert!(
            response
                .headers()
                .get(header::CONTENT_SECURITY_POLICY)
                .is_some()
        );
    }
}
