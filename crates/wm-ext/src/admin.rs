//! Remote control-plane administration, not a remote shell.
//!
//! Sessions stay in memory unless explicitly saved. Persisted sessions are bearer
//! credentials: keep their private file on a trusted local filesystem. Every
//! request is bound to the exact server selected at login; redirects are disabled.
//! Extension catalog/reload/config/activation operate on manifests already present
//! on that server. They do not install files or execute arbitrary remote shell commands.
//! Cookie/CSRF credentials are not interchangeable with node or service tokens.
use std::{fmt, io::{Read, Write}, path::Path, time::Duration};

use reqwest::{Method, Response, header::{COOKIE, SET_COOKIE, HeaderValue}};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use url::Url;
use uuid::Uuid;
use wm_proto::{AdminUser, Overview, NodeView, UpdateNodeRequest, NodeRequirements, AclView,
    SimulateRequest, SimulateResponse, JoinTokenView, CreateJoinTokenRequest,
    ServiceTokenView, CreateServiceTokenRequest, LoginResponse};

pub type Result<T> = std::result::Result<T, AdminError>;

#[derive(Debug, thiserror::Error)]
pub enum AdminError {
    #[error("invalid admin server URL or transport policy")]
    InvalidServer,
    #[error("invalid admin client limits")]
    InvalidLimits,
    #[error("session belongs to a different server")]
    ServerMismatch,
    #[error("invalid session credentials")]
    InvalidSession,
    #[error("invalid extension identifier")]
    InvalidExtension,
    #[error("admin HTTP transport failed")]
    Transport(#[source] reqwest::Error),
    #[error("admin server returned HTTP {status}")]
    Http { status: u16 },
    #[error("admin response exceeds configured limit")]
    ResponseTooLarge,
    #[error("invalid admin JSON response")]
    Json(#[from] serde_json::Error),
    #[error("session file I/O failed")]
    Io(#[from] std::io::Error),
    #[error("session files require a regular, owner-only Unix file")]
    UnsafeSessionFile,
}

#[derive(Clone, Debug)]
pub struct AdminClientOptions {
    /// Explicit consent to plaintext credentials over an operator-trusted overlay.
    /// This does not detect or verify that the destination is on an overlay.
    pub allow_insecure_http_overlay: bool,
    pub timeout: Duration,
    pub max_response_bytes: usize,
}
impl Default for AdminClientOptions {
    fn default() -> Self {
        Self { allow_insecure_http_overlay: false, timeout: Duration::from_secs(20), max_response_bytes: 4 * 1024 * 1024 }
    }
}

/// Secrets are intentionally private and omitted from Debug. No password is retained.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSession {
    server: String,
    cookie: String,
    csrf_token: String,
    user: AdminUser,
}
impl fmt::Debug for AdminSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AdminSession").field("server", &self.server).field("credentials", &"[REDACTED]").finish()
    }
}
impl AdminSession {
    pub fn server(&self) -> &str { &self.server }
    pub fn user(&self) -> &AdminUser { &self.user }

    fn validate(&self) -> Result<()> {
        if !self.cookie.starts_with("wm_session=") || self.cookie.len() > 4096
            || self.cookie[11..].is_empty() || self.cookie.contains(';')
            || self.cookie.bytes().any(|b| b <= 32 || b >= 127)
            || self.csrf_token.is_empty() || self.csrf_token.len() > 4096
            || HeaderValue::from_str(&self.csrf_token).is_err() {
            return Err(AdminError::InvalidSession);
        }
        Ok(())
    }

    /// Explicit persistence only. Never overwrites an existing file or symlink.
    /// Non-Unix platforms fail closed until equivalent ACL checks are implemented.
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        #[cfg(unix)] {
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new().write(true).create_new(true)
                .mode(0o600).custom_flags(libc::O_NOFOLLOW).open(path)?;
            file.write_all(&serde_json::to_vec(self)?)?;
            file.sync_all()?;
            Ok(())
        }
        #[cfg(not(unix))] { let _ = path; Err(AdminError::UnsafeSessionFile) }
    }

    /// `expected_server` is mandatory; loading never silently chooses a server.
    pub fn load(path: &Path, expected_server: &str) -> Result<Self> {
        #[cfg(unix)] {
            use std::os::unix::fs::{OpenOptionsExt, MetadataExt};
            let mut file = std::fs::OpenOptions::new().read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path)?;
            let meta = file.metadata()?;
            if !meta.is_file() || meta.mode() & 0o077 != 0
                || meta.uid() != unsafe { libc::geteuid() } {
                return Err(AdminError::UnsafeSessionFile);
            }
            let mut bytes = Vec::new();
            (&mut file).take(16385).read_to_end(&mut bytes)?;
            if bytes.len() > 16384 { return Err(AdminError::ResponseTooLarge); }
            let session: Self = serde_json::from_slice(&bytes)?;
            session.validate()?;
            // Transport opt-in is enforced again when constructing the client.
            let expected = server_url(expected_server, true)?;
            if session.server != expected.as_str() { return Err(AdminError::ServerMismatch); }
            Ok(session)
        }
        #[cfg(not(unix))] { let _ = (path, expected_server); Err(AdminError::UnsafeSessionFile) }
    }
}

fn server_url(server: &str, allow_http: bool) -> Result<Url> {
    let mut url = Url::parse(server).map_err(|_| AdminError::InvalidServer)?;
    if !url.username().is_empty() || url.password().is_some() || url.query().is_some()
        || url.fragment().is_some() || url.host().is_none() || !matches!(url.path(), "" | "/") {
        return Err(AdminError::InvalidServer);
    }
    // Reject even empty userinfo, which URL normalization would otherwise hide.
    if server.split("://").nth(1).and_then(|s| s.split('/').next()).is_some_and(|s| s.contains('@')) {
        return Err(AdminError::InvalidServer);
    }
    let loopback = match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        None => false,
    };
    if url.scheme() != "https" && !(url.scheme() == "http" && (loopback || allow_http)) {
        return Err(AdminError::InvalidServer);
    }
    url.set_path("/");
    Ok(url)
}

#[derive(Clone)]
pub struct AdminClient {
    server: Url,
    http: reqwest::Client,
    max_response_bytes: usize,
}
impl fmt::Debug for AdminClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AdminClient").field("server", &self.server.as_str()).finish()
    }
}
impl AdminClient {
    pub fn new(server: &str) -> Result<Self> { Self::with_options(server, AdminClientOptions::default()) }
    pub fn with_options(server: &str, options: AdminClientOptions) -> Result<Self> {
        let server = server_url(server, options.allow_insecure_http_overlay)?;
        if options.timeout.is_zero() || options.timeout > Duration::from_secs(120)
            || options.max_response_bytes == 0 || options.max_response_bytes > 16 * 1024 * 1024 {
            return Err(AdminError::InvalidLimits);
        }
        let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none())
            .timeout(options.timeout).connect_timeout(options.timeout.min(Duration::from_secs(10)))
            .referer(false).no_proxy().build().map_err(AdminError::Transport)?;
        Ok(Self { server, http, max_response_bytes: options.max_response_bytes })
    }
    pub fn server(&self) -> &str { self.server.as_str() }

    async fn decode<T: DeserializeOwned>(&self, mut response: Response) -> Result<T> {
        let status = response.status();
        if !status.is_success() { return Err(AdminError::Http { status: status.as_u16() }); }
        if response.content_length().is_some_and(|n| n > self.max_response_bytes as u64) {
            return Err(AdminError::ResponseTooLarge);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(AdminError::Transport)? {
            if chunk.len() > self.max_response_bytes.saturating_sub(bytes.len()) {
                return Err(AdminError::ResponseTooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(serde_json::from_slice(&bytes)?)
    }
    pub async fn login(&self, username: &str, password: &str) -> Result<AdminSession> {
        let response = self.http.post(self.server.join("api/v1/admin/login").expect("fixed path"))
            .json(&json!({"username": username, "password": password})).send().await.map_err(AdminError::Transport)?;
        let cookies: Vec<_> = response.headers().get_all(SET_COOKIE).iter()
            .filter_map(|v| v.to_str().ok()).filter_map(|s| s.split(';').next())
            .filter(|s| s.starts_with("wm_session=")).map(str::to_owned).collect();
        let body: LoginResponse = self.decode(response).await?;
        if cookies.len() != 1 { return Err(AdminError::InvalidSession); }
        let session = AdminSession { server: self.server().to_owned(), cookie: cookies[0].clone(), csrf_token: body.csrf_token, user: body.user };
        session.validate()?;
        Ok(session)
    }
    // Private: callers cannot supply arbitrary methods, paths or origins.
    async fn request<T: DeserializeOwned>(&self, session: &AdminSession, method: Method, path: &str, body: Option<Value>) -> Result<T> {
        if session.server != self.server() { return Err(AdminError::ServerMismatch); }
        session.validate()?;
        let mut cookie = HeaderValue::from_str(&session.cookie).map_err(|_| AdminError::InvalidSession)?;
        cookie.set_sensitive(true);
        let mut csrf = HeaderValue::from_str(&session.csrf_token).map_err(|_| AdminError::InvalidSession)?;
        csrf.set_sensitive(true);
        let mut request = self.http.request(method.clone(), self.server.join(&format!("api/v1/admin/{path}")).expect("allowlisted path"))
            .header(COOKIE, cookie);
        if method != Method::GET { request = request.header("x-wm-csrf", csrf); }
        if let Some(body) = body { request = request.json(&body); }
        self.decode(request.send().await.map_err(AdminError::Transport)?).await
    }
    pub async fn logout(&self, s: &AdminSession) -> Result<Value> { self.request(s, Method::POST, "logout", None).await }
    pub async fn overview(&self, s: &AdminSession) -> Result<Overview> { self.request(s, Method::GET, "overview", None).await }
    pub async fn nodes(&self, s: &AdminSession) -> Result<Vec<NodeView>> { self.request(s, Method::GET, "nodes", None).await }
    pub async fn approve_node(&self, s: &AdminSession, id: Uuid) -> Result<NodeView> { self.request(s, Method::POST, &format!("nodes/{id}/approve"), None).await }
    pub async fn revoke_node(&self, s: &AdminSession, id: Uuid) -> Result<Value> { self.request(s, Method::DELETE, &format!("nodes/{id}"), None).await }
    pub async fn update_node(&self, s: &AdminSession, id: Uuid, req: &UpdateNodeRequest) -> Result<NodeView> { self.request(s, Method::PATCH, &format!("nodes/{id}"), Some(serde_json::to_value(req)?)).await }
    pub async fn set_node_requirements(&self, s: &AdminSession, id: Uuid, req: &NodeRequirements) -> Result<NodeView> { self.request(s, Method::PUT, &format!("nodes/{id}/requirements"), Some(serde_json::to_value(req)?)).await }
    pub async fn acl(&self, s: &AdminSession) -> Result<AclView> { self.request(s, Method::GET, "acl", None).await }
    pub async fn put_acl(&self, s: &AdminSession, yaml: &str) -> Result<AclView> { self.request(s, Method::PUT, "acl", Some(json!({"yaml":yaml}))).await }
    pub async fn validate_acl(&self, s: &AdminSession, yaml: &str) -> Result<Value> { self.request(s, Method::POST, "acl/validate", Some(json!({"yaml":yaml}))).await }
    pub async fn simulate_acl(&self, s: &AdminSession, req: &SimulateRequest) -> Result<SimulateResponse> { self.request(s, Method::POST, "acl/simulate", Some(serde_json::to_value(req)?)).await }
    pub async fn join_tokens(&self, s: &AdminSession) -> Result<Vec<JoinTokenView>> { self.request(s, Method::GET, "join-tokens", None).await }
    pub async fn create_join_token(&self, s: &AdminSession, req: &CreateJoinTokenRequest) -> Result<JoinTokenView> { self.request(s, Method::POST, "join-tokens", Some(serde_json::to_value(req)?)).await }
    pub async fn revoke_join_token(&self, s: &AdminSession, id: Uuid) -> Result<Value> { self.request(s, Method::DELETE, &format!("join-tokens/{id}"), None).await }
    pub async fn service_tokens(&self, s: &AdminSession) -> Result<Vec<ServiceTokenView>> { self.request(s, Method::GET, "service-tokens", None).await }
    pub async fn create_service_token(&self, s: &AdminSession, req: &CreateServiceTokenRequest) -> Result<ServiceTokenView> { self.request(s, Method::POST, "service-tokens", Some(serde_json::to_value(req)?)).await }
    pub async fn revoke_service_token(&self, s: &AdminSession, id: Uuid) -> Result<Value> { self.request(s, Method::DELETE, &format!("service-tokens/{id}"), None).await }
    pub async fn extensions(&self, s: &AdminSession) -> Result<Value> { self.request(s, Method::GET, "extensions", None).await }
    pub async fn extension(&self, s: &AdminSession, id: &str) -> Result<Value> { self.request(s, Method::GET, &extension_path(id, None, "")?, None).await }
    pub async fn reload_extensions(&self, s: &AdminSession) -> Result<Value> { self.request(s, Method::POST, "extensions/reload", None).await }
    pub async fn extension_config(&self, s: &AdminSession, id: &str, node: Uuid) -> Result<Value> { self.request(s, Method::GET, &extension_path(id, Some(node), "config")?, None).await }
    pub async fn put_extension_config(&self, s: &AdminSession, id: &str, node: Uuid, enabled: bool, config: &Value) -> Result<Value> { self.request(s, Method::PUT, &extension_path(id, Some(node), "config")?, Some(json!({"enabled":enabled,"config":config}))).await }
    pub async fn delete_extension_config(&self, s: &AdminSession, id: &str, node: Uuid) -> Result<Value> { self.request(s, Method::DELETE, &extension_path(id, Some(node), "config")?, None).await }
    pub async fn set_extension_activation(&self, s: &AdminSession, id: &str, node: Uuid, active: bool) -> Result<Value> { self.request(s, Method::PUT, &extension_path(id, Some(node), "activation")?, Some(json!({"active":active}))).await }
}
fn extension_path(id: &str, node: Option<Uuid>, suffix: &str) -> Result<String> {
    if !crate::manifest::valid_id(id) || matches!(id, "." | "..") { return Err(AdminError::InvalidExtension); }
    Ok(match node { Some(node) => format!("extensions/{id}/nodes/{node}/{suffix}"), None => format!("extensions/{id}") })
}


#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn session(server: &str) -> AdminSession {
        AdminSession { server: server.into(), cookie: "wm_session=secret".into(), csrf_token: "csrf-secret".into(),
            user: AdminUser { id: Uuid::nil(), username: "admin".into() } }
    }

    #[test]
    fn url_transport_policy() {
        for url in ["https://user:pass@example.org", "https://@example.org", "https://example.org?x=1",
            "https://example.org#fragment", "https://example.org/api/v1", "ftp://example.org",
            "http://example.org", "http://127.0.0.1.evil.invalid", "http://10.0.0.1"] {
            assert!(AdminClient::new(url).is_err(), "accepted {url}");
        }
        for url in ["https://example.org", "http://127.0.0.1:8080", "http://[::1]:8080", "http://localhost:8080"] {
            assert!(AdminClient::new(url).is_ok(), "rejected {url}");
        }
        assert!(AdminClient::with_options("http://10.0.0.1", AdminClientOptions { allow_insecure_http_overlay: true, ..Default::default() }).is_ok());
        assert!(AdminClient::with_options("https://example.org", AdminClientOptions { timeout: Duration::ZERO, ..Default::default() }).is_err());
    }

    #[test]
    fn extension_paths_cannot_escape_allowlist() {
        for id in ["..", ".", "/overview", "foo/bar", "foo%2fbar", "foo?x=1", "foo#frag", "https://evil"] {
            assert!(extension_path(id, Some(Uuid::nil()), "config").is_err());
        }
        assert_eq!(extension_path("cloudflared", None, "").unwrap(), "extensions/cloudflared");
    }

    #[test]
    fn credentials_are_redacted_and_bound_to_server() {
        let s = session("https://example.org/");
        let debug = format!("{s:?}");
        assert!(!debug.contains("secret"));
        let mut invalid = s.clone();
        invalid.cookie = "wm_session=secret; stolen=token".into();
        assert!(invalid.validate().is_err());
        invalid.cookie = "wm_session=secret\r\nx-evil: 1".into();
        assert!(invalid.validate().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn persistence_checks_permissions_symlinks_and_server() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("session.json");
        let s = session("https://example.org/");
        s.save(&path).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(AdminSession::load(&path, "https://example.org").unwrap().server(), s.server());
        assert!(matches!(AdminSession::load(&path, "https://other.org"), Err(AdminError::ServerMismatch)));
        assert!(s.save(&path).is_err());
        let link = temp.path().join("link");
        symlink(&path, &link).unwrap();
        assert!(AdminSession::load(&link, "https://example.org").is_err());
        assert!(s.save(&link).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(AdminSession::load(&path, "https://example.org"), Err(AdminError::UnsafeSessionFile)));
        assert!(AdminSession::load(temp.path(), "https://example.org").is_err());
    }

    async fn mock(responses: Vec<String>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server = format!("http://{}/", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let mut requests = Vec::new();
            for response in responses {
                let (mut stream, _) = tokio::time::timeout(Duration::from_secs(3), listener.accept()).await.unwrap().unwrap();
                let mut bytes = Vec::new();
                loop {
                    let mut buf = [0; 4096];
                    let n = stream.read(&mut buf).await.unwrap();
                    assert_ne!(n, 0);
                    bytes.extend_from_slice(&buf[..n]);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                        let length = head.lines().find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap())).unwrap_or(0);
                        if bytes.len() >= end + 4 + length { break; }
                    }
                }
                requests.push(String::from_utf8(bytes).unwrap());
                stream.write_all(response.as_bytes()).await.unwrap();
            }
            requests
        });
        (server, task)
    }
    fn response(body: &str, extra: &str) -> String {
        format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n{body}", body.len())
    }

    #[tokio::test]
    async fn login_cookie_csrf_and_real_allowlisted_routes() {
        let login = json!({"user":{"id":Uuid::nil(),"username":"admin"},"csrf_token":"csrf-secret"}).to_string();
        let (server, task) = mock(vec![response(&login, "Set-Cookie: wm_session=secret; HttpOnly; Path=/\r\n"), response("[]", ""), response("{\"ok\":true}", "")]).await;
        let client = AdminClient::new(&server).unwrap();
        let s = client.login("admin", "password-never-retained").await.unwrap();
        client.extensions(&s).await.unwrap();
        client.revoke_node(&s, Uuid::nil()).await.unwrap();
        let requests = task.await.unwrap();
        assert!(requests[0].starts_with("POST /api/v1/admin/login "));
        assert!(!requests[0].to_ascii_lowercase().contains("cookie:"));
        assert!(requests[1].starts_with("GET /api/v1/admin/extensions "));
        assert!(requests[1].to_ascii_lowercase().contains("cookie: wm_session=secret"));
        assert!(!requests[1].to_ascii_lowercase().contains("x-wm-csrf:"));
        assert!(requests[2].starts_with(&format!("DELETE /api/v1/admin/nodes/{} ", Uuid::nil())));
        assert!(requests[2].to_ascii_lowercase().contains("x-wm-csrf: csrf-secret"));
        let other = AdminClient::new("https://other.invalid").unwrap();
        assert!(matches!(other.extensions(&s).await, Err(AdminError::ServerMismatch)));
    }

    #[tokio::test]
    async fn login_requires_cookie_and_requests_have_deadlines() {
        let login = json!({"user":{"id":Uuid::nil(),"username":"admin"},"csrf_token":"csrf-secret"}).to_string();
        let (server, task) = mock(vec![response(&login, "")]).await;
        let client = AdminClient::new(&server).unwrap();
        assert!(matches!(client.login("admin", "password").await, Err(AdminError::InvalidSession)));
        task.await.unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server = format!("http://{}/", listener.local_addr().unwrap());
        let client = AdminClient::with_options(&server, AdminClientOptions {
            timeout: Duration::from_millis(30), ..Default::default()
        }).unwrap();
        // Keep the listening socket open without answering: the request must
        // finish under the configured deadline rather than wait for a body.
        assert!(matches!(client.extensions(&session(&server)).await, Err(AdminError::Transport(ref e)) if e.is_timeout()));
        drop(listener);
    }

    #[tokio::test]
    async fn refuses_redirects_and_oversized_responses() {
        let (server, task) = mock(vec!["HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/steal\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into()]).await;
        let client = AdminClient::new(&server).unwrap();
        assert!(matches!(client.extensions(&session(&server)).await, Err(AdminError::Http { status: 302 })));
        task.await.unwrap();
        let (server, task) = mock(vec![response("[1,2,3,4,5]", "")]).await;
        let client = AdminClient::with_options(&server, AdminClientOptions { max_response_bytes: 4, ..Default::default() }).unwrap();
        assert!(matches!(client.extensions(&session(&server)).await, Err(AdminError::ResponseTooLarge)));
        task.await.unwrap();
        let (server, task) = mock(vec!["HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\nb\r\n[1,2,3,4,5]\r\n0\r\n\r\n".into()]).await;
        let client = AdminClient::with_options(&server, AdminClientOptions { max_response_bytes: 4, ..Default::default() }).unwrap();
        assert!(matches!(client.extensions(&session(&server)).await, Err(AdminError::ResponseTooLarge)));
        task.await.unwrap();
    }
}
