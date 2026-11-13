//! Local member GUI (`wm gui`).
//!
//! A small HTTP server bound to loopback that serves one embedded page and
//! proxies the local agent API. It is deliberately separate from the control
//! panel: members of a mesh do not have the admin password, but they do have
//! the node token in the local socket. The public page contains no credential:
//! a random capability is delivered in a URL fragment and required by bootstrap
//! and every API call, with exact Host/Origin validation.

use std::net::SocketAddr;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use axum::extract::{Path, State, Request};
use axum::middleware::{self, Next};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use wm_core::state::{AgentConfig, NodeState};
use wm_proto::{
    CreateShareRequest, LocalRequest, LocalResponse, RenameSelfRequest, UpdateShareRequest,
    decode_join_target,
};

const GUI_HTML: &str = include_str!("gui.html");
const GUI_TOKEN_HEADER: &str = "x-wm-gui-token";

struct GuiState {
    data_root: PathBuf,
    socket: PathBuf,
    state_path: PathBuf,
    token: String,
    authority: String,
}

type ApiError = (StatusCode, Json<Value>);

fn api_error(status: StatusCode, message: impl Into<String>) -> ApiError {
    (status, Json(json!({ "error": { "message": message.into() } })))
}

fn authorize(headers: &HeaderMap, state: &GuiState) -> Result<(), ApiError> {
    let Some(value) = headers.get(GUI_TOKEN_HEADER).and_then(|v| v.to_str().ok()) else {
        return Err(api_error(StatusCode::FORBIDDEN, "token da GUI ausente"));
    };
    if value != state.token {
        return Err(api_error(StatusCode::FORBIDDEN, "token da GUI inválido"));
    }
    Ok(())
}

pub async fn run(data_root: PathBuf, listen: SocketAddr, open: bool) -> anyhow::Result<()> {
    validate_listen(listen)?;
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("bind da GUI em {listen}"))?;
    let listen = listener.local_addr()?;
    let token = wm_control::auth::random_token("wmgui_")?;
    let socket = data_root.join("agent.sock");
    let state_path = data_root.join("agent-state.json");
    let state = Arc::new(GuiState {
        data_root,
        socket,
        state_path,
        token,
        authority: listen.to_string(),
    });

    let app = router(state.clone());
    let url = format!("http://{listen}/#t={}", state.token);
    println!("wired-magic GUI: http://{listen}/ (capacidade necessária)");
    println!("  (mantenha este processo rodando; Ctrl+C encerra)");
    if open {
        open_browser(&url);
    } else if let Ok(mut tty) = std::fs::OpenOptions::new().write(true).open("/dev/tty") {
        // Deliberate credential delivery to the terminal, never redirected logs.
        let _ = writeln!(tty, "Abra a URL privada (não compartilhe): {url}");
    } else {
        println!("Execute com --open para abrir a URL privada no navegador.");
    }
    axum::serve(listener, app).await?;
    Ok(())
}

fn validate_listen(listen: SocketAddr) -> anyhow::Result<()> {
    anyhow::ensure!(listen.ip().is_loopback(), "a GUI exige bind loopback");
    Ok(())
}

fn router(state: Arc<GuiState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/state", get(api_state))
        .route("/api/shares", post(create_share))
        .route(
            "/api/shares/{id}",
            patch(update_share).delete(delete_share),
        )
        .route("/api/rename", post(rename_node))
        .route("/api/join", post(join_mesh))
        .route("/api/bootstrap", post(bootstrap))
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

fn check_context(headers: &HeaderMap, state: &GuiState) -> Result<(), ApiError> {
    let host = headers.get_all(header::HOST);
    if host.iter().count() != 1 || host.iter().next().and_then(|v| v.to_str().ok()) != Some(state.authority.as_str()) {
        return Err(api_error(StatusCode::FORBIDDEN, "Host da GUI inválido"));
    }
    let expected = format!("http://{}", state.authority);
    let origins = headers.get_all(header::ORIGIN);
    if origins.iter().count() > 1 || origins.iter().next().is_some_and(|v| v.to_str().ok() != Some(expected.as_str())) {
        return Err(api_error(StatusCode::FORBIDDEN, "Origin da GUI inválida"));
    }
    if headers.get("sec-fetch-site").is_some_and(|v| v != "same-origin" && v != "none") {
        return Err(api_error(StatusCode::FORBIDDEN, "requisição cross-origin negada"));
    }
    Ok(())
}

async fn guard(State(state): State<Arc<GuiState>>, request: Request, next: Next) -> Response {
    let authorization = check_context(request.headers(), &state).and_then(|()| {
        if request.uri().path().starts_with("/api/") {
            authorize(request.headers(), &state)
        } else {
            Ok(())
        }
    });
    let mut response = match authorization {
        Ok(()) => next.run(request).await,
        Err(error) => error.into_response(),
    };
    for (name, value) in [
        ("cache-control", "no-store, max-age=0"),
        ("referrer-policy", "no-referrer"),
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "DENY"),
        ("content-security-policy", "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'"),
    ] {
        response.headers_mut().insert(axum::http::HeaderName::from_static(name), value.parse().unwrap());
    }
    response
}

async fn bootstrap(State(state): State<Arc<GuiState>>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    authorize(&headers, &state)?;
    Ok(Json(json!({ "ok": true })))
}

fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(target_os = "linux")]
    let program = "xdg-open";
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let program = "";
    if program.is_empty() {
        return;
    }
    let _ = std::process::Command::new(program)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

async fn index() -> Response {
    let mut response = Html(GUI_HTML).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        "no-store, max-age=0".parse().expect("cache header"),
    );
    response
}

async fn api_state(
    State(state): State<Arc<GuiState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    authorize(&headers, &state)?;

    let status = local_call(&state.socket, "status", Value::Null).await;
    let shares = local_call(&state.socket, "shares", Value::Null).await;
    let extensions = local_call(&state.socket, "extensions", Value::Null).await;
    let node = match NodeState::load(&state.state_path) {
        Ok(Some(node)) => Some(node),
        _ => None,
    };
    let online = status.is_ok();
    let status_value = status.as_ref().ok();
    let error = status.as_ref().err().cloned();
    let name = status_value
        .and_then(|status| status.get("node_name"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| node.as_ref().map(|node| node.node_name.clone()));
    let overlay_ip = status_value
        .and_then(|status| status.get("overlay_ip"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| node.as_ref().map(|node| node.overlay_ip.to_string()));
    Ok(Json(json!({
        "onboarding": {
            "enrolled": node.is_some(),
            "command": start_command(&state.data_root),
        },
        "refresh": {
            "healthy": online && shares.is_ok() && extensions.is_ok(),
            "errors": {
                "status": error,
                "shares": shares.as_ref().err(),
                "extensions": extensions.as_ref().err(),
            },
        },
        "agent": {
            "online": online,
            "status": status_value,
            "error": error,
        },
        "node": {
            "name": name,
            "overlay_ip": overlay_ip,
            "control_url": node.as_ref().map(|node| node.control_url.clone()),
        },
        "shares": shares.ok(),
        "extensions": extensions.ok(),
    })))
}

#[derive(Debug, Deserialize)]
struct JoinInput {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

async fn join_mesh(
    State(state): State<Arc<GuiState>>,
    headers: HeaderMap,
    Json(input): Json<JoinInput>,
) -> Result<Json<Value>, ApiError> {
    authorize(&headers, &state)?;
    if state.state_path.exists() {
        return Err(api_error(
            StatusCode::CONFLICT,
            "este nó já está inscrito; remova o estado para entrar em outra malha",
        ));
    }
    let (url, token) = match (input.code.as_deref(), input.url.as_deref()) {
        (Some(code), _) if !code.trim().is_empty() => {
            let (url, token) = decode_join_target(code.trim())
                .map_err(|err| api_error(StatusCode::BAD_REQUEST, err))?;
            let token = token.ok_or_else(|| {
                api_error(StatusCode::BAD_REQUEST, "o código não contém um token")
            })?;
            (url, token)
        }
        (_, Some(url)) if !url.trim().is_empty() => {
            let token = input
                .token
                .as_deref()
                .map(str::trim)
                .filter(|token| !token.is_empty())
                .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "informe o token wmj_"))?;
            (url.trim().to_string(), token.to_string())
        }
        _ => {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "informe um código ou URL + token",
            ));
        }
    };

    let cfg = AgentConfig::with_data_dir(state.data_root.clone());
    let node = wm_core::join(&cfg, &url, &token, input.name.as_deref())
        .await
        .map_err(|err| api_error(StatusCode::BAD_GATEWAY, format!("falha ao entrar: {err:#}")))?;
    Ok(Json(json!({
        "ok": true,
        "node_name": node.node_name,
        "overlay_ip": node.overlay_ip,
        "control_url": node.control_url,
        "command": start_command(&state.data_root),
    })))
}

// Display only: the browser never executes this command or elevates privileges.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn start_command(data_root: &std::path::Path) -> String {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("wm"));
    // Resolve relative roots before sudo changes the user's default state directory.
    let root = if data_root.is_absolute() { data_root.to_path_buf() } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(data_root)
    };
    format!("sudo -- {} --data-dir {} up", shell_quote(&exe.to_string_lossy()), shell_quote(&root.to_string_lossy()))
}

async fn create_share(
    State(state): State<Arc<GuiState>>,
    headers: HeaderMap,
    Json(input): Json<CreateShareRequest>,
) -> Result<Json<Value>, ApiError> {
    authorize(&headers, &state)?;
    let params = serde_json::to_value(&input)
        .map_err(|err| api_error(StatusCode::BAD_REQUEST, err.to_string()))?;
    proxy(&state.socket, "share_create", params).await
}

async fn update_share(
    State(state): State<Arc<GuiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<UpdateShareRequest>,
) -> Result<Json<Value>, ApiError> {
    authorize(&headers, &state)?;
    let mut params = serde_json::to_value(&input)
        .map_err(|err| api_error(StatusCode::BAD_REQUEST, err.to_string()))?;
    if let Some(object) = params.as_object_mut() {
        object.insert("id".into(), Value::String(id));
    }
    proxy(&state.socket, "share_update", params).await
}

async fn delete_share(
    State(state): State<Arc<GuiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    authorize(&headers, &state)?;
    proxy(&state.socket, "share_delete", json!({ "id": id })).await
}

async fn rename_node(
    State(state): State<Arc<GuiState>>,
    headers: HeaderMap,
    Json(input): Json<RenameSelfRequest>,
) -> Result<Json<Value>, ApiError> {
    authorize(&headers, &state)?;
    let params = serde_json::to_value(&input)
        .map_err(|err| api_error(StatusCode::BAD_REQUEST, err.to_string()))?;
    proxy(&state.socket, "rename_node", params).await
}

async fn proxy(socket: &PathBuf, method: &str, params: Value) -> Result<Json<Value>, ApiError> {
    let value = local_call(socket, method, params)
        .await
        .map_err(|err| api_error(StatusCode::BAD_GATEWAY, err))?;
    Ok(Json(value))
}

async fn local_call(socket: &PathBuf, method: &str, params: Value) -> Result<Value, String> {
    tokio::time::timeout(std::time::Duration::from_secs(2), local_call_inner(socket, method, params))
        .await.map_err(|_| format!("tempo esgotado ao consultar {method}"))?
}

async fn local_call_inner(socket: &PathBuf, method: &str, params: Value) -> Result<Value, String> {
    let stream = UnixStream::connect(socket)
        .await
        .map_err(|err| format!("agente não está ouvindo em {}: {err}", socket.display()))?;
    let (read, mut write) = stream.into_split();
    let req = LocalRequest {
        id: 1,
        method: method.to_string(),
        params,
    };
    let mut line = serde_json::to_vec(&req).map_err(|err| err.to_string())?;
    line.push(b'\n');
    write
        .write_all(&line)
        .await
        .map_err(|err| err.to_string())?;
    write.flush().await.ok();
    let mut reader = BufReader::new(read);
    let mut response = String::new();
    reader
        .read_line(&mut response)
        .await
        .map_err(|err| err.to_string())?;
    let parsed: LocalResponse =
        serde_json::from_str(&response).map_err(|err| format!("resposta inválida: {err}"))?;
    if parsed.ok {
        Ok(parsed.result.unwrap_or(Value::Null))
    } else {
        Err(parsed.error.unwrap_or_else(|| "erro no agente".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> Arc<GuiState> {
        Arc::new(GuiState {
            data_root: PathBuf::new(), socket: PathBuf::new(), state_path: PathBuf::new(),
            token: "test-secret".into(), authority: "127.0.0.1:9876".into(),
        })
    }

    #[tokio::test]
    async fn http_guard_denies_cross_origin_and_rebinding() {
        use tokio::io::AsyncReadExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut state = state();
        Arc::get_mut(&mut state).unwrap().authority = address.to_string();
        let task = tokio::spawn(async move { axum::serve(listener, router(state)).await.unwrap() });
        for (path, host, origin, token, expected) in [
            ("/", address.to_string(), "", "", "200"),
            ("/api/bootstrap", address.to_string(), "", "", "403"),
            ("/api/bootstrap", address.to_string(), "", "test-secret", "200"),
            ("/api/bootstrap", address.to_string(), "http://evil.example", "test-secret", "403"),
            ("/api/state", address.to_string(), "http://evil.example", "test-secret", "403"),
            ("/", "evil.example".into(), "", "", "403"),
        ] {
            let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
            let method = if path == "/api/bootstrap" { "POST" } else { "GET" };
            let origin = if origin.is_empty() { String::new() } else { format!("Origin: {origin}\r\n") };
            stream.write_all(format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\n{origin}x-wm-gui-token: {token}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).await.unwrap();
            assert!(response.starts_with(&format!("HTTP/1.1 {expected}")), "{response}");
            assert!(!response.contains("test-secret"));
            if expected == "200" {
                assert!(response.contains("cache-control: no-store"));
                assert!(response.contains("referrer-policy: no-referrer"));
            }
        }
        task.abort();
    }

    #[test]
    fn startup_command_quotes_paths_and_keeps_explicit_data_dir() {
        assert_eq!(shell_quote("/tmp/a b'$(id)"), "'/tmp/a b'\"'\"'$(id)'");
        let command = start_command(std::path::Path::new("/tmp/member's state"));
        assert!(command.starts_with("sudo -- '"));
        assert!(command.ends_with(" --data-dir '/tmp/member'\"'\"'s state' up"));
        assert!(!command.contains("sh -c"));
    }

    #[test]
    fn html_keeps_startup_separate_from_hidden_enrollment_form() {
        let command = GUI_HTML.find("id=\"start-command\"").unwrap();
        let form = GUI_HTML.find("id=\"connect-panel\"").unwrap();
        assert!(command < form);
        for contract in ["state.enrolled = !!data.onboarding.enrolled", "state.command = data.onboarding.command",
            "state.degraded = true", "state.agent = null", "state.sharesAvailable = false",
            "Falha de atualização:", "Última consulta completa:", "Catálogo indisponível",
            "if (refreshing) return", "finally", "AbortSignal.timeout(8000)", "inscrito como"] {
            assert!(GUI_HTML.contains(contract), "missing HTML contract: {contract}");
        }
        assert!(!GUI_HTML.contains("conectado como"));
        assert!(!GUI_HTML.contains("sudo wm up"));
    }

    #[tokio::test]
    async fn http_state_offline_is_degraded_not_empty_healthy() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut state = state();
        Arc::get_mut(&mut state).unwrap().authority = address.to_string();
        let task = tokio::spawn(async move { axum::serve(listener, router(state)).await.unwrap() });
        use tokio::io::AsyncReadExt;
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        stream.write_all(format!("GET /api/state HTTP/1.1\r\nHost: {address}\r\nx-wm-gui-token: test-secret\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        assert!(response.starts_with("HTTP/1.1 200"));
        let body: Value = serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body["agent"]["online"], false);
        assert_eq!(body["refresh"]["healthy"], false);
        assert!(body["refresh"]["errors"]["shares"].is_string());
        assert!(body["shares"].is_null());
        assert!(body["extensions"].is_null());
        assert_eq!(body["onboarding"]["enrolled"], false);
        assert!(body["onboarding"]["command"].as_str().unwrap().contains(" --data-dir "));
        task.abort();
    }

    #[tokio::test]
    async fn enrolled_refresh_retains_command_and_reports_partial_failure() {
        let root = std::env::temp_dir().join(format!("wm-gui-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir(&root).unwrap();
        let state_path = root.join("agent-state.json");
        std::fs::write(&state_path, json!({
            "node_id": "00000000-0000-0000-0000-000000000001", "node_token": "private-node-token",
            "overlay_ip": "100.64.0.1", "wg_private_key": "private-key", "wg_public_key": "public-key",
            "control_url": "https://control.example", "node_name": "member", "network_name": "test",
            "created_at": "2026-01-01T00:00:00Z"
        }).to_string()).unwrap();
        let socket = root.join("agent.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let mut state = state();
        let mutable = Arc::get_mut(&mut state).unwrap();
        mutable.data_root = root.clone();
        mutable.state_path = state_path.clone();
        mutable.socket = socket.clone();
        let agent = tokio::spawn(async move {
            for round in 0..2 {
                for method in ["status", "shares", "extensions"] {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let mut line = String::new();
                    BufReader::new(&mut stream).read_line(&mut line).await.unwrap();
                    let request: Value = serde_json::from_str(&line).unwrap();
                    assert_eq!(request["method"], method);
                    let response = if round == 1 && method == "shares" {
                        json!({"id":1, "ok":false, "error":"catalog unavailable"})
                    } else {
                        json!({"id":1, "ok":true, "result": if method == "status" {
                            json!({"node_name":"member", "control_connected":true})
                        } else { json!([]) }})
                    };
                    stream.write_all(format!("{response}\n").as_bytes()).await.unwrap();
                }
            }
        });
        let mut headers = HeaderMap::new();
        headers.insert(GUI_TOKEN_HEADER, "test-secret".parse().unwrap());
        let healthy = api_state(State(state.clone()), headers.clone()).await.unwrap().0;
        assert_eq!(healthy["refresh"]["healthy"], true);
        assert_eq!(healthy["onboarding"]["enrolled"], true);
        let partial = api_state(State(state.clone()), headers.clone()).await.unwrap().0;
        assert_eq!(partial["agent"]["online"], true);
        assert_eq!(partial["refresh"]["healthy"], false);
        assert!(partial["shares"].is_null());
        assert_eq!(partial["refresh"]["errors"]["shares"], "catalog unavailable");
        agent.await.unwrap();
        let offline = api_state(State(state), headers).await.unwrap().0;
        assert_eq!(offline["agent"]["online"], false);
        assert_eq!(offline["onboarding"], healthy["onboarding"]);
        assert!(!offline.to_string().contains("private-node-token"));
        assert!(!offline.to_string().contains("private-key"));
        // All paths were created in this test's unique directory above.
        std::fs::remove_file(state_path).unwrap();
        std::fs::remove_file(socket).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn bind_is_loopback_only() {
        for addr in ["0.0.0.0:0", "192.168.1.1:80", "[::]:0"] {
            assert!(validate_listen(addr.parse().unwrap()).is_err());
        }
        for addr in ["127.0.0.1:0", "[::1]:0"] {
            assert!(validate_listen(addr.parse().unwrap()).is_ok());
        }
    }

    #[test]
    fn origin_and_host_are_checked_including_port() {
        let state = state();
        let mut headers = HeaderMap::new();
        assert!(check_context(&headers, &state).is_err());
        headers.insert(header::HOST, "127.0.0.1:9876".parse().unwrap());
        assert!(check_context(&headers, &state).is_ok());
        for origin in ["null", "http://evil.example", "http://127.0.0.1:9877", "https://127.0.0.1:9876"] {
            headers.insert(header::ORIGIN, origin.parse().unwrap());
            assert!(check_context(&headers, &state).is_err());
        }
        headers.insert(header::ORIGIN, "http://127.0.0.1:9876".parse().unwrap());
        assert!(check_context(&headers, &state).is_ok());
        headers.insert("sec-fetch-site", "cross-site".parse().unwrap());
        assert!(check_context(&headers, &state).is_err());
        headers.remove("sec-fetch-site");
        headers.insert(header::HOST, "evil.example:9876".parse().unwrap());
        assert!(check_context(&headers, &state).is_err());
    }

    #[tokio::test]
    async fn anonymous_html_has_no_secret_and_bootstrap_requires_capability() {
        let state = state();
        let response = index().await;
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(!String::from_utf8(body.to_vec()).unwrap().contains(&state.token));
        let mut headers = HeaderMap::new();
        assert!(bootstrap(State(state.clone()), headers.clone()).await.is_err());
        assert!(api_state(State(state.clone()), headers.clone()).await.is_err());
        headers.insert(GUI_TOKEN_HEADER, "wrong".parse().unwrap());
        assert!(bootstrap(State(state.clone()), headers.clone()).await.is_err());
        headers.insert(GUI_TOKEN_HEADER, state.token.parse().unwrap());
        assert!(bootstrap(State(state), headers).await.is_ok());
    }
}
