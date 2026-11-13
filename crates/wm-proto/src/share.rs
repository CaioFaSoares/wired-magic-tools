//! Named services ("shares") published on the mesh, plus the opaque connect
//! code used by the one-line onboarding flow.
//!
//! The control plane owns the registry; every agent receives the enabled and
//! disabled shares in its network map and resolves `wm://name` locally. A
//! share does not move packets by itself: the peer connects to the owner's
//! overlay address and the WireGuard data plane carries the traffic.

use std::fmt;
use std::net::Ipv4Addr;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_SHARE_NAME: usize = 63;
pub const MAX_SHARE_DESCRIPTION: usize = 240;
pub const CONNECT_CODE_PREFIX: &str = "wmc_";

/// What the service is, for presentation and the "open" action.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareKind {
    #[default]
    Web,
    Tcp,
    Udp,
    Game,
}

impl ShareKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ShareKind::Web => "web",
            ShareKind::Tcp => "tcp",
            ShareKind::Udp => "udp",
            ShareKind::Game => "game",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "web" | "http" | "https" => Some(ShareKind::Web),
            "tcp" | "port" => Some(ShareKind::Tcp),
            "udp" => Some(ShareKind::Udp),
            "game" | "jogo" | "server" | "servidor" => Some(ShareKind::Game),
            _ => None,
        }
    }

    pub fn is_web(self) -> bool {
        matches!(self, ShareKind::Web)
    }
}

impl fmt::Display for ShareKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Transport of the published service. A game server commonly needs both.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareProtocol {
    #[default]
    Tcp,
    Udp,
    Both,
}

impl ShareProtocol {
    pub fn as_str(self) -> &'static str {
        match self {
            ShareProtocol::Tcp => "tcp",
            ShareProtocol::Udp => "udp",
            ShareProtocol::Both => "both",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "tcp" => Some(ShareProtocol::Tcp),
            "udp" => Some(ShareProtocol::Udp),
            "both" | "tcp+udp" | "tcp,udp" | "all" => Some(ShareProtocol::Both),
            _ => None,
        }
    }

    pub fn includes_tcp(self) -> bool {
        matches!(self, ShareProtocol::Tcp | ShareProtocol::Both)
    }

    pub fn includes_udp(self) -> bool {
        matches!(self, ShareProtocol::Udp | ShareProtocol::Both)
    }
}

impl fmt::Display for ShareProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One service published by a node. `overlay_ip` is copied from the owner node
/// when the view is built so the GUI/TUI can show a ready-to-use address.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareInfo {
    pub id: Uuid,
    pub name: String,
    pub node_id: Uuid,
    pub node_name: String,
    pub overlay_ip: Ipv4Addr,
    pub port: u16,
    pub kind: ShareKind,
    pub protocol: ShareProtocol,
    #[serde(default)]
    pub description: String,
    pub enabled: bool,
    pub online: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ShareInfo {
    /// `10.66.0.5:3000` for TCP/UDP/game and for web (the caller adds a scheme).
    pub fn address(&self) -> String {
        format!("{}:{}", self.overlay_ip, self.port)
    }

    /// Browser URL for a web share.
    pub fn web_url(&self) -> String {
        format!("http://{}:{}/", self.overlay_ip, self.port)
    }

    pub fn wm_url(&self) -> String {
        format!("wm://{}", self.name)
    }

    /// The most useful URL for the panel: the cloudflared public URL when
    /// present, otherwise the overlay URL for web shares.
    pub fn open_url(&self) -> Option<String> {
        if let Some(public) = &self.public_url {
            return Some(public.clone());
        }
        self.kind.is_web().then(|| self.web_url())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateShareRequest {
    pub name: String,
    pub port: u16,
    #[serde(default)]
    pub kind: ShareKind,
    #[serde(default)]
    pub protocol: ShareProtocol,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl CreateShareRequest {
    /// Applies the kind-specific defaults so an omitted protocol does the
    /// expected thing (`web` -> tcp, `game` -> both).
    pub fn normalized(mut self) -> Self {
        self.protocol = match self.kind {
            ShareKind::Web => ShareProtocol::Tcp,
            ShareKind::Udp => ShareProtocol::Udp,
            ShareKind::Game if self.protocol == ShareProtocol::Tcp => ShareProtocol::Both,
            _ => self.protocol,
        };
        self
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateShareRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub kind: Option<ShareKind>,
    #[serde(default)]
    pub protocol: Option<ShareProtocol>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminCreateShareRequest {
    pub node_id: Uuid,
    #[serde(flatten)]
    pub share: CreateShareRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameSelfRequest {
    pub name: String,
}

/// Validates a share name: lowercase slug, starting alphanumeric.
pub fn validate_share_name(raw: &str) -> Result<String, String> {
    let value = raw.trim().to_ascii_lowercase();
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_SHARE_NAME {
        return Err(format!(
            "nome do share deve ter 1-{MAX_SHARE_NAME} caracteres"
        ));
    }
    if !bytes[0].is_ascii_alphanumeric() {
        return Err("nome do share deve começar com letra ou número".into());
    }
    if !bytes
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || *b == b'-')
    {
        return Err("nome do share usa apenas letras, números e '-'".into());
    }
    Ok(value)
}

pub fn validate_share_description(raw: &str) -> Result<String, String> {
    let value = raw.trim();
    if value.chars().count() > MAX_SHARE_DESCRIPTION {
        return Err(format!(
            "descrição do share deve ter no máximo {MAX_SHARE_DESCRIPTION} caracteres"
        ));
    }
    if value.chars().any(char::is_control) {
        return Err("descrição do share contém caracteres de controle".into());
    }
    Ok(value.to_string())
}

// ---------------------------------------------------------------------------
// Connect codes
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
struct ConnectCodePayload {
    v: u8,
    url: String,
    token: String,
}

/// Opaque, pasteable code carrying the control URL and a join token. The token
/// itself is only stored hashed by the control plane; the code is what the
/// operator sends to another person.
pub fn encode_connect_code(control_url: &str, token: &str) -> Result<String, String> {
    let url = control_url.trim().trim_end_matches('/');
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("URL do control deve começar com http:// ou https://".into());
    }
    if token.trim().is_empty() {
        return Err("token de convite vazio".into());
    }
    let payload = ConnectCodePayload {
        v: 1,
        url: url.to_string(),
        token: token.trim().to_string(),
    };
    let json = serde_json::to_vec(&payload).map_err(|err| err.to_string())?;
    Ok(format!(
        "{CONNECT_CODE_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(json)
    ))
}

/// Decodes a `wmc_...` code into `(control_url, join_token)`.
pub fn decode_connect_code(raw: &str) -> Result<(String, String), String> {
    let value = raw.trim();
    let encoded = value
        .strip_prefix(CONNECT_CODE_PREFIX)
        .ok_or_else(|| format!("código inválido: comece com {CONNECT_CODE_PREFIX}"))?;
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| "código inválido: base64url quebrado".to_string())?;
    let payload: ConnectCodePayload =
        serde_json::from_slice(&bytes).map_err(|_| "código inválido: conteúdo desconhecido".to_string())?;
    if payload.v != 1 {
        return Err(format!("código de versão {} não suportado", payload.v));
    }
    if !(payload.url.starts_with("https://") || payload.url.starts_with("http://")) {
        return Err("código inválido: URL do control desconhecida".into());
    }
    if payload.token.trim().is_empty() {
        return Err("código inválido: token vazio".into());
    }
    Ok((payload.url, payload.token))
}

/// Accepts a `wmc_` code or a `wm://join?...` URL. The latter is useful when a
/// human pastes a link rather than an opaque code.
pub fn decode_join_target(raw: &str) -> Result<(String, Option<String>), String> {
    let value = raw.trim();
    if value.starts_with(CONNECT_CODE_PREFIX) {
        let (url, token) = decode_connect_code(value)?;
        return Ok((url, Some(token)));
    }
    if let Some(rest) = value.strip_prefix("wm://join?") {
        let mut url = None;
        let mut token = None;
        for part in rest.split('&') {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            let decoded = percent_decode(value);
            match key {
                "url" | "server" => url = Some(decoded),
                "token" | "code" => token = Some(decoded),
                _ => {}
            }
        }
        let url = url.ok_or_else(|| "link sem parâmetro url".to_string())?;
        return Ok((url, token));
    }
    Err("formato não reconhecido: use um código wmc_... ou um link wm://join?...".into())
}

fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_descriptions_are_strict() {
        assert_eq!(validate_share_name("Demo-1").unwrap(), "demo-1");
        for bad in ["", "-demo", "démo", "a b", "a/b", &"a".repeat(64)] {
            assert!(validate_share_name(bad).is_err(), "aceitou {bad}");
        }
        assert!(validate_share_description("um preview").is_ok());
        assert!(validate_share_description(&"a".repeat(241)).is_err());
        assert!(validate_share_description("a\u{0}b").is_err());
    }

    #[test]
    fn kind_defaults_follow_the_protocol() {
        let web = CreateShareRequest {
            name: "app".into(),
            port: 3000,
            kind: ShareKind::Web,
            protocol: ShareProtocol::Both,
            description: String::new(),
            enabled: true,
        }
        .normalized();
        assert_eq!(web.protocol, ShareProtocol::Tcp);

        let game = CreateShareRequest {
            name: "cs2".into(),
            port: 27015,
            kind: ShareKind::Game,
            protocol: ShareProtocol::Tcp,
            description: String::new(),
            enabled: true,
        }
        .normalized();
        assert_eq!(game.protocol, ShareProtocol::Both);

        let udp = CreateShareRequest {
            name: "voice".into(),
            port: 9999,
            kind: ShareKind::Udp,
            protocol: ShareProtocol::Both,
            description: String::new(),
            enabled: true,
        }
        .normalized();
        assert_eq!(udp.protocol, ShareProtocol::Udp);
    }

    #[test]
    fn connect_code_roundtrips() {
        let code = encode_connect_code("https://mesh.example.com/", "wmj_abc").unwrap();
        assert!(code.starts_with("wmc_"));
        assert!(!code.contains("wmj_abc"));
        assert_eq!(
            decode_connect_code(&code).unwrap(),
            ("https://mesh.example.com".into(), "wmj_abc".into())
        );
        assert!(decode_connect_code("nope").is_err());
        assert!(decode_connect_code("wmc_not-base64!").is_err());
    }

    #[test]
    fn join_link_is_accepted() {
        let (url, token) =
            decode_join_target("wm://join?url=https%3A%2F%2Fmesh.example.com&token=wmj_x").unwrap();
        assert_eq!(url, "https://mesh.example.com");
        assert_eq!(token.as_deref(), Some("wmj_x"));
    }

    #[test]
    fn share_view_has_ready_addresses() {
        let share = ShareInfo {
            id: Uuid::new_v4(),
            name: "demo".into(),
            node_id: Uuid::new_v4(),
            node_name: "dev".into(),
            overlay_ip: "10.66.0.5".parse().unwrap(),
            port: 3000,
            kind: ShareKind::Web,
            protocol: ShareProtocol::Tcp,
            description: String::new(),
            enabled: true,
            online: true,
            public_url: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        assert_eq!(share.address(), "10.66.0.5:3000");
        assert_eq!(share.web_url(), "http://10.66.0.5:3000/");
        assert_eq!(share.wm_url(), "wm://demo");
        assert_eq!(share.open_url().as_deref(), Some("http://10.66.0.5:3000/"));
    }
}
