//! HTTP debug dashboard; collection belongs to core.
use super::core::*;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use anyhow::Context;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use serde_json::{Value, json};
use crate::discovery::DiscoverySnapshot;
const INDEX_HTML: &str = include_str!("../../debug_assets/index.html");
const APP_CSS: &str = include_str!("../../debug_assets/app.css");
const APP_JS: &str = include_str!("../../debug_assets/app.js");
#[derive(Debug, Serialize)]
struct InfoResponse {
    title: String,
    node: Option<String>,
    version: String,
    interval_secs: u64,
}

// ---------------------------------------------------------------------------
// State and sampler
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct DebugState {
    title: String,
    node: Option<String>,
    snapshot: Arc<RwLock<Option<Arc<Snapshot>>>>,
}

impl DebugState {
    fn new(title: String, node: Option<String>, first: Snapshot) -> Self {
        Self {
            title,
            node,
            snapshot: Arc::new(RwLock::new(Some(Arc::new(first)))),
        }
    }

    fn set(&self, snapshot: Snapshot) {
        if let Ok(mut current) = self.snapshot.write() {
            *current = Some(Arc::new(snapshot));
        }
    }

    fn get(&self) -> Option<Arc<Snapshot>> {
        self.snapshot
            .read()
            .ok()
            .and_then(|current| current.clone())
    }
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
    Json(json!({ "ok": true, "service": "wm-debug" }))
}

async fn info(State(state): State<DebugState>) -> Json<InfoResponse> {
    Json(InfoResponse {
        title: state.title.clone(),
        node: state.node.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        interval_secs: SAMPLE_INTERVAL.as_secs(),
    })
}

async fn extensions(State(state): State<DebugState>) -> Json<DiscoverySnapshot> {
    Json(
        state
            .get()
            .map(|snapshot| snapshot.extensions.clone())
            .unwrap_or_else(DiscoverySnapshot::empty),
    )
}

async fn snapshot(State(state): State<DebugState>) -> Response {
    match state.get() {
        Some(snapshot) => Json((*snapshot).clone()).into_response(),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "coletando a primeira amostra" })),
        )
            .into_response(),
    }
}

async fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "error": "rota não encontrada" })),
    )
        .into_response()
}

fn app(state: DebugState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/index.html", get(index))
        .route("/app.css", get(css))
        .route("/app.js", get(js))
        .route("/healthz", get(health))
        .route("/api/info", get(info))
        .route("/api/snapshot", get(snapshot))
        .route("/api/extensions", get(extensions))
        .fallback(not_found)
        .layer(DefaultBodyLimit::max(8 * 1024))
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

/// Starts the sampler and serves the dashboard until the process is stopped.
pub async fn run(
    root: PathBuf,
    listen: SocketAddr,
    title: String,
    node: Option<String>,
) -> anyhow::Result<()> {
    let mut sampler = Sampler::new(&root)?;
    let first = sampler.sample();
    let state = DebugState::new(title, node, first);

    let sampler_state = state.clone();
    std::thread::Builder::new()
        .name("wm-debug-sampler".into())
        .spawn(move || {
            loop {
                std::thread::sleep(SAMPLE_INTERVAL);
                sampler_state.set(sampler.sample());
            }
        })
        .context("não foi possível iniciar o coletor de métricas")?;

    let app = app(state);
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("não foi possível escutar em {listen}"))?;
    println!("debug_gui: ouvindo em http://{listen}");
    axum::serve(listener, app)
        .await
        .context("servidor de debug encerrou com erro")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use axum::body::{Body, to_bytes};
    use serde_json::Value as JsonValue;
    use tower::ServiceExt;

    fn test_state() -> DebugState {
        // A tiny synthetic snapshot keeps the HTTP tests independent from the
        // host's real load.
        let snapshot = SurrogateSnapshot::build();
        DebugState::new("Debug de teste".into(), Some("nó-1".into()), snapshot)
    }

    struct SurrogateSnapshot;

    impl SurrogateSnapshot {
        fn build() -> Snapshot {
            Snapshot {
                generated_at: Utc::now(),
                host: HostInfo {
                    hostname: "host".into(),
                    name: "OS".into(),
                    os_version: "1".into(),
                    long_os_version: "OS 1".into(),
                    kernel: "kernel".into(),
                    arch: "arm64".into(),
                    uptime_secs: 42,
                },
                cpu: CpuInfo {
                    brand: "CPU".into(),
                    cores: 2,
                    physical_cores: Some(2),
                    frequency_mhz: 1000,
                    usage_percent: 10.0,
                    per_core: vec![10.0, 10.0],
                },
                memory: MemoryInfo {
                    total_bytes: 100,
                    used_bytes: 40,
                    available_bytes: 60,
                    swap_total_bytes: 0,
                    swap_used_bytes: 0,
                },
                load: LoadInfo {
                    one: 0.1,
                    five: 0.1,
                    fifteen: 0.1,
                },
                disks: Vec::new(),
                networks: Vec::new(),
                wm: None,
                wm_source: "unavailable",
                processes: Vec::new(),
                extensions: DiscoverySnapshot::empty(),
            }
        }
    }

    async fn body_json(response: Response) -> JsonValue {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn dashboard_and_snapshot_are_served_with_security_headers() {
        let app = app(test_state());

        let response = app
            .clone()
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
        let info = body_json(response).await;
        assert_eq!(info["title"], "Debug de teste");
        assert_eq!(info["node"], "nó-1");
        assert_eq!(info["interval_secs"], 1);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/snapshot")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let snapshot = body_json(response).await;
        assert_eq!(snapshot["host"]["hostname"], "host");
        assert_eq!(snapshot["cpu"]["cores"], 2);
        assert_eq!(snapshot["memory"]["used_bytes"], 40);
    }
}
