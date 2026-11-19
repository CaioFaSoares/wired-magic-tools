//! One-time bootstrap and human sessions. Neither implies node enrollment.
use std::sync::Arc;

use axum::Json;
use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Response};
use wm_proto::{
    AccountLoginResponse, LoginRequest, SetupCompleteRequest, SetupCompleteResponse,
    SetupLoginResponse, SetupStatus,
};

use super::{ApiErr, client_ip, client_ua};
use crate::{auth, db, state::AppState};

pub const SETUP_COOKIE: &str = "wm_setup";
pub const SETUP_TTL_SECS: i64 = 10 * 60;

/// Shared bound for newly chosen permanent and bootstrap credentials. Legacy
/// login passwords remain valid even if shorter than today's creation minimum.
pub(crate) fn validate_new_credentials(username: &str, password: &str) -> Result<(), ApiErr> {
    if username.is_empty() || username.len() > 64 || !username.bytes().all(|c|
        c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
    {
        return Err(ApiErr::bad_request("invalid_username", "username exige 1..64 caracteres ASCII: letras, números, _, - ou ."));
    }
    if !(12..=1024).contains(&password.len()) {
        return Err(ApiErr::bad_request("invalid_password", "senha nova exige 12..1024 bytes"));
    }
    Ok(())
}

fn invalid_credentials() -> ApiErr {
    ApiErr::unauthorized("invalid_credentials", "usuário ou senha inválidos")
}

fn check_login_input(req: &LoginRequest) -> Result<(), ApiErr> {
    if req.username.trim().is_empty() || req.username.len() > 64 || req.password.is_empty() || req.password.len() > 1024 {
        return Err(invalid_credentials());
    }
    Ok(())
}

fn check_rate(state: &AppState, headers: &HeaderMap) -> Result<(), ApiErr> {
    let ip = client_ip(headers).unwrap_or_else(|| "unknown".into());
    if !state.login_limiter.allow(&ip) {
        return Err(ApiErr::too_many("muitas tentativas de autenticação"));
    }
    Ok(())
}

pub struct AccountAuth {
    pub user: db::AdminRow,
    pub session: db::SessionRow,
}

impl AccountAuth {
    pub fn require_csrf(&self, headers: &HeaderMap) -> Result<(), ApiErr> {
        require_csrf(headers, &self.session.csrf_token)
    }
}

fn require_csrf(headers: &HeaderMap, expected: &str) -> Result<(), ApiErr> {
    let token = headers.get("x-wm-csrf").and_then(|v| v.to_str().ok())
        .ok_or_else(|| ApiErr::forbidden("csrf_missing", "cabeçalho X-WM-CSRF ausente"))?;
    if token != expected {
        return Err(ApiErr::forbidden("csrf_invalid", "CSRF inválido"));
    }
    Ok(())
}

impl FromRequestParts<Arc<AppState>> for AccountAuth {
    type Rejection = ApiErr;

    async fn from_request_parts(parts: &mut Parts, state: &Arc<AppState>) -> Result<Self, Self::Rejection> {
        let token = auth::parse_cookie(&parts.headers, auth::SESSION_COOKIE)
            .filter(|token| token.len() <= 256)
            .ok_or_else(|| ApiErr::unauthorized("not_authenticated", "sessão ausente"))?;
        let session = db::session_by_hash(&state.db, &auth::hash_token(&token)).await?
            .ok_or_else(|| ApiErr::unauthorized("session_expired", "sessão expirada"))?;
        let user = db::admin_by_id(&state.db, &session.user_id).await?
            .ok_or_else(|| ApiErr::unauthorized("not_authenticated", "conta não encontrada"))?;
        // Fail closed for unknown future roles, not a default-to-admin conversion.
        user.account()?;
        Ok(Self { user, session })
    }
}

pub async fn setup_status(State(state): State<Arc<AppState>>) -> Result<Json<SetupStatus>, ApiErr> {
    Ok(Json(SetupStatus { required: db::setup_required(&state.db).await? }))
}

fn setup_cookie(value: &str, max_age: i64, secure: bool) -> String {
    format!("{SETUP_COOKIE}={value}; Path=/api/v1/setup; HttpOnly; SameSite=Strict; Max-Age={max_age}{}",
        if secure { "; Secure" } else { "" })
}

fn setup_closed() -> ApiErr {
    ApiErr::conflict("setup_complete", "configuração inicial já concluída")
}

pub async fn setup_login(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> Result<Response, ApiErr> {
    check_rate(&state, &headers)?;
    check_login_input(&req)?;
    let bootstrap = db::bootstrap(&state.db).await?.ok_or_else(setup_closed)?;
    let password = req.password;
    let hash = bootstrap.password_hash.clone();
    let permit = state.password_work.clone().acquire_owned().await
        .map_err(|_| ApiErr::internal("autenticação indisponível"))?;
    let valid = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        auth::verify_password(&password, &hash)
    }).await.map_err(|_| ApiErr::internal("falha de autenticação"))?;
    if !valid || req.username.trim() != bootstrap.username {
        return Err(invalid_credentials());
    }
    let token = auth::random_token("wmsetup_")?;
    let csrf = auth::random_token("wmcsrf_")?;
    if !db::create_setup_session(&state.db, &bootstrap.password_hash, &auth::hash_token(&token), &csrf, SETUP_TTL_SECS).await? {
        return Err(setup_closed());
    }
    let mut response = Json(SetupLoginResponse { csrf_token: csrf }).into_response();
    response.headers_mut().insert(header::SET_COOKIE,
        setup_cookie(&token, SETUP_TTL_SECS, state.is_https()).parse().expect("setup cookie"));
    response.headers_mut().insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    Ok(response)
}

pub async fn setup_complete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<SetupCompleteRequest>,
) -> Result<Response, ApiErr> {
    if !db::setup_required(&state.db).await? { return Err(setup_closed()); }
    let token = auth::parse_cookie(&headers, SETUP_COOKIE)
        .filter(|token| token.len() <= 256)
        .ok_or_else(|| ApiErr::unauthorized("setup_session_missing", "sessão bootstrap ausente"))?;
    let token_hash = auth::hash_token(&token);
    let session = db::setup_session_by_hash(&state.db, &token_hash).await?
        .ok_or_else(|| ApiErr::unauthorized("setup_session_expired", "sessão bootstrap expirada"))?;
    require_csrf(&headers, &session.csrf_token)?;
    validate_new_credentials(&req.admin.username, &req.admin.password)?;
    validate_new_credentials(&req.user.username, &req.user.password)?;
    if req.admin.username.eq_ignore_ascii_case(&req.user.username) {
        return Err(ApiErr::bad_request("duplicate_username", "admin e usuário devem ter usernames distintos"));
    }
    // Completion also consumes the authentication rate budget, even with a
    // valid setup cookie, before allocating memory for two Argon2 operations.
    check_rate(&state, &headers)?;
    let admin_name = req.admin.username;
    let user_name = req.user.username;
    let admin_password = req.admin.password;
    let user_password = req.user.password;
    let permit = state.password_work.clone().acquire_owned().await
        .map_err(|_| ApiErr::internal("autenticação indisponível"))?;
    let (admin_hash, user_hash) = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let _permit = permit;
        Ok((auth::hash_password(&admin_password)?, auth::hash_password(&user_password)?))
    }).await.map_err(|_| ApiErr::internal("falha no hash de senha"))??;
    let result = db::complete_setup(&state.db, &token_hash, &session.csrf_token, &admin_name, &admin_hash, &user_name, &user_hash).await;
    let completed: SetupCompleteResponse = match result {
        Ok(Some(completed)) => completed,
        Ok(None) => return Err(ApiErr::conflict("setup_session_changed", "setup concluído ou sessão bootstrap não é mais válida")),
        Err(err) => {
            if err.downcast_ref::<sqlx::Error>().is_some_and(|e| matches!(e,
                sqlx::Error::Database(e) if e.is_unique_violation()))
            {
                return Err(ApiErr::conflict("username_taken", "username já existe"));
            }
            return Err(err.into());
        }
    };
    // No implicit permanent login: the setup credential has no authority over
    // either account after commit, and ALL setup sessions were deleted.
    let mut response = Json(completed).into_response();
    response.headers_mut().insert(header::SET_COOKIE,
        setup_cookie("", 0, state.is_https()).parse().expect("clear setup cookie"));
    response.headers_mut().insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    Ok(response)
}

/// Used by both login APIs; the legacy route must NEVER create user sessions.
pub(super) async fn authenticate(
    state: &Arc<AppState>, headers: &HeaderMap, req: LoginRequest, admin_only: bool,
) -> Result<db::AdminRow, ApiErr> {
    check_rate(state, headers)?;
    check_login_input(&req)?;
    let user = db::admin_by_username(&state.db, req.username.trim()).await?
        .ok_or_else(invalid_credentials)?;
    if admin_only && user.role != "admin" { return Err(invalid_credentials()); }
    user.account()?;
    let password = req.password;
    let hash = user.password_hash.clone();
    let permit = state.password_work.clone().acquire_owned().await
        .map_err(|_| ApiErr::internal("autenticação indisponível"))?;
    let valid = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        auth::verify_password(&password, &hash)
    }).await.map_err(|_| ApiErr::internal("falha de autenticação"))?;
    if !valid { return Err(invalid_credentials()); }
    Ok(user)
}

/// Returns token+CSRF only after an atomic post-hash identity/role recheck.
pub(super) async fn issue_session(state: &AppState, headers: &HeaderMap, user: &db::AdminRow) -> Result<(String, String), ApiErr> {
    let token = auth::random_token("wmsess_")?;
    let csrf = auth::random_token("wmcsrf_")?;
    if !db::create_account_session(&state.db, user, &auth::hash_token(&token), &csrf,
        client_ip(headers).as_deref(), client_ua(headers).as_deref()).await?
    { return Err(invalid_credentials()); }
    Ok((token, csrf))
}

pub async fn login(
    State(state): State<Arc<AppState>>, headers: HeaderMap, Json(req): Json<LoginRequest>,
) -> Result<Response, ApiErr> {
    let user = authenticate(&state, &headers, req, false).await?;
    let (token, csrf) = issue_session(&state, &headers, &user).await?;
    let mut response = Json(AccountLoginResponse { user: user.account()?, csrf_token: csrf }).into_response();
    response.headers_mut().insert(header::SET_COOKIE,
        auth::session_cookie(&token, auth::SESSION_TTL_SECS, state.is_https()).parse().expect("session cookie"));
    response.headers_mut().insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    Ok(response)
}

pub async fn session(account: AccountAuth) -> Result<Response, ApiErr> {
    let mut response = Json(AccountLoginResponse {
        user: account.user.account()?, csrf_token: account.session.csrf_token,
    }).into_response();
    response.headers_mut().insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    Ok(response)
}

pub async fn logout(
    State(state): State<Arc<AppState>>, account: AccountAuth, headers: HeaderMap,
) -> Result<Response, ApiErr> {
    account.require_csrf(&headers)?;
    db::delete_session(&state.db, &account.session.id).await?;
    let mut response = Json(serde_json::json!({"ok": true})).into_response();
    response.headers_mut().insert(header::SET_COOKIE,
        auth::clear_session_cookie(state.is_https()).parse().expect("clear session cookie"));
    response.headers_mut().insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    Ok(response)
}
