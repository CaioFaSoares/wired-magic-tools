pub mod admin;
pub mod install;
pub mod integration;
pub mod node;
pub mod relay_ws;
pub mod shares;
pub mod spa;

use std::sync::Arc;

use axum::Json;
use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use axum::{Router, middleware};
use tower_http::trace::TraceLayer;

use wm_proto::{ApiError, ApiErrorBody, VersionInfo};

use crate::auth;
use crate::db;
use crate::state::AppState;

#[derive(Clone)]
pub struct ApiContext {
    pub state: Arc<AppState>,
}

/// Admin session, extracted from the HttpOnly cookie.
pub struct AdminAuth {
    pub admin: db::AdminRow,
    pub session: db::SessionRow,
}

impl AdminAuth {
    pub fn username(&self) -> &str {
        &self.admin.username
    }

    pub fn id(&self) -> String {
        self.admin.id.clone()
    }

    pub fn require_csrf(&self, headers: &HeaderMap) -> Result<(), ApiErr> {
        let Some(token) = headers.get("x-wm-csrf").and_then(|v| v.to_str().ok()) else {
            return Err(ApiErr::forbidden(
                "csrf_missing",
                "cabeçalho X-WM-CSRF ausente",
            ));
        };
        if token != self.session.csrf_token {
            return Err(ApiErr::forbidden("csrf_invalid", "CSRF inválido"));
        }
        Ok(())
    }
}

impl FromRequestParts<Arc<AppState>> for AdminAuth {
    type Rejection = ApiErr;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let cookie = auth::parse_cookie(&parts.headers, auth::SESSION_COOKIE)
            .ok_or_else(|| ApiErr::unauthorized("not_authenticated", "sessão ausente"))?;
        let hash = auth::hash_token(&cookie);
        let session = db::session_by_hash(&state.db, &hash)
            .await
            .map_err(ApiErr::from)?
            .ok_or_else(|| ApiErr::unauthorized("session_expired", "sessão expirada"))?;
        let admin = db::admin_by_id(&state.db, &session.user_id)
            .await
            .map_err(ApiErr::from)?
            .ok_or_else(|| ApiErr::unauthorized("not_authenticated", "admin não encontrado"))?;
        Ok(Self { admin, session })
    }
}

/// Node bearer token.
pub struct NodeAuth {
    pub node: db::NodeRow,
    pub token_hash: String,
}

impl FromRequestParts<Arc<AppState>> for NodeAuth {
    type Rejection = ApiErr;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let token = auth::bearer_token(&parts.headers)
            .ok_or_else(|| ApiErr::unauthorized("node_token_missing", "token do nó ausente"))?;
        let token_hash = auth::hash_token(token);
        let node = db::node_by_token_hash(&state.db, &token_hash)
            .await
            .map_err(ApiErr::from)?
            .ok_or_else(|| ApiErr::unauthorized("node_token_invalid", "token do nó inválido"))?;
        if node.status() == wm_proto::NodeStatus::Disabled {
            return Err(ApiErr::forbidden("node_disabled", "nó revogado"));
        }
        if node.status() != wm_proto::NodeStatus::Pending {
            db::touch_node(&state.db, &node.id, &token_hash)
                .await
                .map_err(ApiErr::from)?;
        }
        Ok(Self { node, token_hash })
    }
}

/// Service token used by the read-only integration API.
pub struct ServiceAuth {
    pub token: db::ServiceTokenRow,
}

impl ServiceAuth {
    pub fn require_scope(&self, scope: &str) -> Result<(), ApiErr> {
        if self.token.scopes().iter().any(|s| s == scope) {
            Ok(())
        } else {
            Err(ApiErr::forbidden("scope_missing", "token sem escopo necessário"))
        }
    }
}

impl FromRequestParts<Arc<AppState>> for ServiceAuth {
    type Rejection = ApiErr;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let token = auth::bearer_token(&parts.headers)
            .ok_or_else(|| ApiErr::unauthorized("service_token_missing", "token de serviço ausente"))?;
        let hash = auth::hash_token(token);
        let row = db::service_token_by_hash(&state.db, &hash)
            .await
            .map_err(ApiErr::from)?
            .ok_or_else(|| ApiErr::unauthorized("service_token_invalid", "token inválido"))?;
        db::touch_service_token(&state.db, &row.id)
            .await
            .map_err(ApiErr::from)?;
        Ok(Self { token: row })
    }
}

#[derive(Debug)]
pub struct ApiErr {
    pub status: StatusCode,
    pub code: String,
    pub message: String,
    pub details: Option<serde_json::Value>,
}

impl ApiErr {
    pub fn new(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        Self {
            status,
            code: code.to_string(),
            message: message.into(),
            details: None,
        }
    }
    pub fn bad_request(code: &str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }
    pub fn unauthorized(code: &str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, code, message)
    }
    pub fn forbidden(code: &str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, code, message)
    }
    pub fn not_found(code: &str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, code, message)
    }
    pub fn conflict(code: &str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, code, message)
    }
    pub fn too_many(message: impl Into<String>) -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited", message)
    }
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
    }
}

impl From<anyhow::Error> for ApiErr {
    fn from(err: anyhow::Error) -> Self {
        tracing::error!(error = %err, "erro interno da API");
        Self::internal("erro interno")
    }
}

impl From<String> for ApiErr {
    fn from(err: String) -> Self {
        Self::bad_request("invalid", err)
    }
}

impl IntoResponse for ApiErr {
    fn into_response(self) -> Response {
        let body = ApiErrorBody {
            error: ApiError {
                code: self.code,
                message: self.message,
                details: self.details,
            },
        };
        (self.status, Json(body)).into_response()
    }
}

// This internal header is always removed/replaced at the router boundary.
// Never derive rate-limit or audit identity directly from client-controlled
// forwarding headers. Behind a reverse proxy, its socket IP shares one bucket.
const SOCKET_IP_HEADER: &str = "x-wm-internal-socket-ip";

pub fn client_ip(headers: &HeaderMap) -> Option<String> {
    headers.get(SOCKET_IP_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<std::net::IpAddr>().ok())
        .map(|ip| ip.to_string())
}

async fn socket_identity(mut req: axum::extract::Request, next: middleware::Next) -> Response {
    let peer = req.extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|info| info.0.ip());
    req.headers_mut().remove(SOCKET_IP_HEADER);
    if let Some(peer) = peer {
        req.headers_mut().insert(SOCKET_IP_HEADER, peer.to_string().parse().expect("IP header"));
    }
    next.run(req).await
}

pub fn client_ua(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.chars().take(200).collect())
}

pub fn router(state: Arc<AppState>) -> Router {
    let api = Router::new()
        .route("/meta", get(meta))
        .route("/enroll", post(node::enroll))
        .route("/map", get(node::map))
        .route("/node/endpoints", post(node::endpoints))
        .route("/node/report", post(node::report))
        .route("/node/leave", post(node::leave))
        .route(
            "/node/shares",
            get(shares::list_own).post(shares::create_own),
        )
        .route(
            "/node/shares/{id}",
            patch(shares::update_own).delete(shares::delete_own),
        )
        .route("/node/self", patch(shares::rename_self))
        .route("/node/profile", patch(shares::update_profile))
        .route("/admin/login", post(admin::login))
        .route("/admin/logout", post(admin::logout))
        .route("/admin/me", get(admin::me))
        .route("/admin/overview", get(admin::overview))
        .route("/admin/nodes", get(admin::list_nodes))
        .route(
            "/admin/nodes/{id}",
            patch(admin::update_node).delete(admin::revoke_node),
        )
        .route("/admin/nodes/{id}/approve", post(admin::approve_node))
        .route(
            "/admin/nodes/{id}/requirements",
            axum::routing::put(admin::set_node_requirements),
        )
        .route(
            "/admin/shares",
            get(shares::list_admin).post(shares::create_admin),
        )
        .route(
            "/admin/shares/{id}",
            patch(shares::update_admin).delete(shares::delete_admin),
        )
        .route(
            "/admin/join-tokens",
            get(admin::list_join_tokens).post(admin::create_join_token),
        )
        .route("/admin/join-tokens/{id}", delete(admin::revoke_join_token))
        .route(
            "/admin/acl",
            get(admin::get_acl).put(admin::put_acl),
        )
        .route("/admin/acl/validate", post(admin::validate_acl))
        .route("/admin/acl/simulate", post(admin::simulate_acl))
        .route("/admin/audit", get(admin::audit_log))
        .route("/admin/events", get(admin::events))
        .route(
            "/admin/service-tokens",
            get(admin::list_service_tokens).post(admin::create_service_token),
        )
        .route(
            "/admin/service-tokens/{id}",
            delete(admin::revoke_service_token),
        )
        .route("/admin/settings", get(admin::settings))
        .route("/admin/session", get(admin::session))
        .route("/integration/nodes", get(integration::nodes))
        .route("/integration/nodes/{name}", get(integration::node))
        .route("/integration/health", get(integration::health));

    let api = api.merge(crate::extensions::router(state.clone()));

    Router::new()
        .route("/healthz", get(health))
        .route("/j/{token}", get(install::script))
        .route("/download/wm", get(install::binary))
        .route("/relay", get(relay_ws::handler))
        .nest("/api/v1", api)
        .fallback(spa::handler)
        .layer(middleware::from_fn(security_headers))
        .layer(middleware::from_fn(socket_identity))
        .layer(axum::extract::DefaultBodyLimit::max(1024 * 1024))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn health(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": wm_proto::PROTOCOL_VERSION,
        "uptime_secs": state.started.elapsed().as_secs(),
    }))
}

async fn meta(State(state): State<Arc<AppState>>) -> Json<VersionInfo> {
    Json(VersionInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        protocol: wm_proto::PROTOCOL_VERSION,
        control_pubkey: state.control_pubkey.clone(),
        network: state.network.clone(),
        relay: state
            .cfg
            .relay_enabled
            .then(|| state.cfg.relay_public_url()),
        stun: state.cfg.stun_urls.clone(),
    })
}

async fn security_headers(
    req: axum::extract::Request,
    next: middleware::Next,
) -> Response {
    let mut response = next.run(req).await;
    let headers = response.headers_mut();
    headers.insert("x-content-type-options", "nosniff".parse().unwrap());
    headers.insert("x-frame-options", "DENY".parse().unwrap());
    headers.insert("referrer-policy", "no-referrer".parse().unwrap());
    headers.insert(
        "content-security-policy",
        "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'"
            .parse()
            .unwrap(),
    );
    response
}
