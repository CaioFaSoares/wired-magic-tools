//! Public chat extension server (`wm chat`).
//!
//! The `chat` extension in `extensions/public/chat` is declarative: its
//! `exec` sidecar runs `wm chat` on the node. This module is the server half —
//! a tiny, dependency-light room with a static UI and a JSON API.
//!
//! Design constraints:
//!
//! * it listens on loopback by default; publishing is the adapter's job
//!   (share/expose) and, optionally, cloudflared's;
//! * there is **no authentication**: anyone who can reach the port can read
//!   and post. That is the point of a public room, and it is documented in the
//!   manifest and README;
//! * messages live in memory and are capped, so a sidecar restart clears the
//!   room. Keeping state out of the control plane avoids a second database and
//!   keeps the public surface small.
//! * user content is rendered with `textContent` in the browser, never as
//!   HTML, and the server rejects control characters except `\n`/`\t`.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::Context;
use axum::extract::{DefaultBodyLimit, Query, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const INDEX_HTML: &str = include_str!("chat_assets/index.html");
const APP_CSS: &str = include_str!("chat_assets/app.css");
const APP_JS: &str = include_str!("chat_assets/app.js");

/// Message cap per sidecar. Older messages are dropped first.
const MAX_MESSAGES: usize = 500;
const MAX_AUTHOR_CHARS: usize = 32;
const MAX_TEXT_CHARS: usize = 2000;
const MAX_BODY_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize)]
struct ChatMessage {
    id: u64,
    author: String,
    text: String,
    at: DateTime<Utc>,
}

#[derive(Debug)]
struct Room {
    title: String,
    node: Option<String>,
    /// Identifies this process instance. A browser that sees a new epoch
    /// clears its local view instead of waiting for ids that restarted at 1.
    epoch: String,
    messages: VecDeque<ChatMessage>,
    next_id: u64,
}

impl Room {
    fn new(title: String, node: Option<String>) -> Self {
        Self {
            title,
            node,
            epoch: uuid::Uuid::new_v4().to_string(),
            messages: VecDeque::new(),
            next_id: 1,
        }
    }

    fn push(&mut self, author: String, text: String) -> ChatMessage {
        let message = ChatMessage {
            id: self.next_id,
            author,
            text,
            at: Utc::now(),
        };
        self.next_id += 1;
        self.messages.push_back(message.clone());
        while self.messages.len() > MAX_MESSAGES {
            self.messages.pop_front();
        }
        message
    }

    fn after(&self, after: u64) -> Vec<ChatMessage> {
        self.messages
            .iter()
            .filter(|message| message.id > after)
            .cloned()
            .collect()
    }
}

#[derive(Clone)]
struct ChatState {
    room: Arc<Mutex<Room>>,
}

impl ChatState {
    fn new(title: String, node: Option<String>) -> Self {
        Self {
            room: Arc::new(Mutex::new(Room::new(title, node))),
        }
    }

    fn room(&self) -> MutexGuard<'_, Room> {
        self.room
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(default)]
    after: u64,
}

#[derive(Debug, Deserialize)]
struct PostInput {
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Serialize)]
struct InfoResponse {
    title: String,
    node: Option<String>,
    epoch: String,
    messages: usize,
    max_messages: usize,
    max_text: usize,
    subtitle: String,
}

#[derive(Debug, Serialize)]
struct MessagesResponse {
    epoch: String,
    cursor: u64,
    messages: Vec<ChatMessage>,
}

#[derive(Debug, Serialize)]
struct PostResponse {
    epoch: String,
    message: ChatMessage,
}

type ApiError = (StatusCode, Json<Value>);

fn bad_request(message: impl Into<String>) -> ApiError {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": message.into() })),
    )
}

fn clean_author(raw: &str) -> Result<String, String> {
    let author = raw.trim();
    let author = if author.is_empty() {
        "anônimo"
    } else {
        author
    };
    if author.chars().count() > MAX_AUTHOR_CHARS {
        return Err(format!(
            "o nome deve ter no máximo {MAX_AUTHOR_CHARS} caracteres"
        ));
    }
    if author.chars().any(char::is_control) {
        return Err("o nome contém caracteres de controle".into());
    }
    Ok(author.to_string())
}

fn clean_text(raw: &str) -> Result<String, String> {
    let text = raw.trim();
    if text.is_empty() {
        return Err("a mensagem não pode estar vazia".into());
    }
    if text.chars().count() > MAX_TEXT_CHARS {
        return Err(format!(
            "a mensagem deve ter no máximo {MAX_TEXT_CHARS} caracteres"
        ));
    }
    if text
        .chars()
        .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\t'))
    {
        return Err("a mensagem contém caracteres de controle".into());
    }
    Ok(text.to_string())
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

async fn info(State(state): State<ChatState>) -> Json<InfoResponse> {
    let room = state.room();
    let subtitle = match &room.node {
        Some(node) => format!(
            "Sala aberta no nó {node}. Qualquer pessoa com acesso a esta porta pode conversar."
        ),
        None => "Sala aberta: qualquer pessoa com acesso a esta porta pode conversar.".into(),
    };
    Json(InfoResponse {
        title: room.title.clone(),
        node: room.node.clone(),
        epoch: room.epoch.clone(),
        messages: room.messages.len(),
        max_messages: MAX_MESSAGES,
        max_text: MAX_TEXT_CHARS,
        subtitle,
    })
}

async fn list_messages(
    State(state): State<ChatState>,
    Query(query): Query<ListQuery>,
) -> Json<MessagesResponse> {
    let room = state.room();
    let messages = room.after(query.after);
    let cursor = messages
        .last()
        .map(|message| message.id)
        .unwrap_or(query.after);
    Json(MessagesResponse {
        epoch: room.epoch.clone(),
        cursor,
        messages,
    })
}

async fn create_message(
    State(state): State<ChatState>,
    Json(input): Json<PostInput>,
) -> Result<(StatusCode, Json<PostResponse>), ApiError> {
    let author = clean_author(input.author.as_deref().unwrap_or("")).map_err(bad_request)?;
    let text = clean_text(input.text.as_deref().unwrap_or("")).map_err(bad_request)?;

    let mut room = state.room();
    let message = room.push(author, text);
    let epoch = room.epoch.clone();
    Ok((StatusCode::CREATED, Json(PostResponse { epoch, message })))
}

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
    println!("chat: ouvindo em http://{listen}");
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

    #[test]
    fn author_defaults_and_rejects_controls() {
        assert_eq!(clean_author("  ").unwrap(), "anônimo");
        assert_eq!(clean_author(" Ana ").unwrap(), "Ana");
        assert!(clean_author("a\u{0}b").is_err());
        assert!(clean_author(&"x".repeat(MAX_AUTHOR_CHARS + 1)).is_err());
        assert!(clean_author(&"x".repeat(MAX_AUTHOR_CHARS)).is_ok());
    }

    #[test]
    fn text_is_trimmed_capped_and_keeps_newlines() {
        assert_eq!(clean_text("  oi\nmundo  ").unwrap(), "oi\nmundo");
        assert!(clean_text("   ").is_err());
        assert!(clean_text("a\u{7}b").is_err());
        assert!(clean_text(&"x".repeat(MAX_TEXT_CHARS + 1)).is_err());
        assert!(clean_text(&"x".repeat(MAX_TEXT_CHARS)).is_ok());
    }

    #[test]
    fn room_filters_after_and_caps_history() {
        let mut room = Room::new("Sala".into(), None);
        for index in 0..(MAX_MESSAGES + 2) {
            room.push("Ana".into(), format!("mensagem {index}"));
        }
        assert_eq!(room.messages.len(), MAX_MESSAGES);
        let after_last = room.after(room.next_id - 1);
        assert!(after_last.is_empty());
        let all = room.after(0);
        assert_eq!(all.first().unwrap().id, 3);
        assert_eq!(all.last().unwrap().id, MAX_MESSAGES as u64 + 2);
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
