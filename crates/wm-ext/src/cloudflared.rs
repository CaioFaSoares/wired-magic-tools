//! Cloudflare Tunnel extension: configuration contracts, validation and the
//! named-tunnel YAML renderer.
//!
//! The control plane owns routes and node settings; the agent only receives a
//! [`CloudflaredConfig`] in the map and supervises `cloudflared` processes.
//!
//! Security decision: a route's `local_url` must point at loopback. The agent
//! runs as root, so allowing arbitrary hosts would turn a mesh node into a
//! pivot that exposes peer services. A project that really lives on another
//! host can expose it locally with its own forward and still use a route.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::Path;

use serde::{Deserialize, Serialize};
use url::{Host, Url};
use uuid::Uuid;

pub const DEFAULT_BINARY: &str = "cloudflared";
pub const MAX_ROUTES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloudflaredMode {
    /// Ephemeral `*.trycloudflare.com` URL, no Cloudflare account required.
    Quick,
    /// Named tunnel with a hostname from the operator's zone, one process per
    /// node and ingress rendered from the routes.
    Named,
}

impl CloudflaredMode {
    pub fn as_str(self) -> &'static str {
        match self {
            CloudflaredMode::Quick => "quick",
            CloudflaredMode::Named => "named",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "quick" => Some(CloudflaredMode::Quick),
            "named" => Some(CloudflaredMode::Named),
            _ => None,
        }
    }
}

/// One exposed project route. `hostname` is required for named routes and
/// ignored for quick routes (Cloudflare assigns the URL).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CloudflaredRouteConfig {
    pub id: String,
    pub project: String,
    pub local_url: String,
    #[serde(default)]
    pub hostname: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CloudflaredTunnelConfig {
    pub tunnel_id: String,
    /// Raw `cloudflared tunnel create` credentials JSON, stored only by the
    /// control plane and delivered to the owning node.
    pub credentials_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CloudflaredConfig {
    #[serde(default = "default_binary")]
    pub binary: String,
    #[serde(default)]
    pub tunnel: Option<CloudflaredTunnelConfig>,
    #[serde(default)]
    pub quick: Vec<CloudflaredRouteConfig>,
    #[serde(default)]
    pub named: Vec<CloudflaredRouteConfig>,
}

fn default_binary() -> String {
    DEFAULT_BINARY.into()
}

impl Default for CloudflaredConfig {
    fn default() -> Self {
        Self {
            binary: default_binary(),
            tunnel: None,
            quick: Vec::new(),
            named: Vec::new(),
        }
    }
}

impl CloudflaredConfig {
    pub fn route_count(&self) -> usize {
        self.quick.len() + self.named.len()
    }

    pub fn is_empty(&self) -> bool {
        self.route_count() == 0
    }

    pub fn validate(&self) -> Result<(), String> {
        validate_binary(&self.binary)?;
        if self.route_count() > MAX_ROUTES {
            return Err(format!("no máximo {MAX_ROUTES} rotas por nó"));
        }
        let mut ids = std::collections::HashSet::new();
        let mut hostnames = BTreeMap::new();
        for route in self.quick.iter().chain(self.named.iter()) {
            if route.id.trim().is_empty() || !ids.insert(route.id.clone()) {
                return Err("id de rota vazio ou duplicado".into());
            }
            validate_project(&route.project)?;
            validate_local_url(&route.local_url)?;
        }
        for route in &self.named {
            let hostname = route
                .hostname
                .as_deref()
                .ok_or_else(|| format!("rota '{}': hostname é obrigatório no modo named", route.project))?;
            let hostname = validate_hostname(hostname)?;
            if let Some(previous) = hostnames.insert(hostname.clone(), route.project.clone()) {
                return Err(format!(
                    "hostname '{hostname}' repetido nas rotas '{previous}' e '{}'",
                    route.project
                ));
            }
        }
        if !self.named.is_empty() {
            let tunnel = self.tunnel.as_ref().ok_or_else(|| {
                "rotas named exigem o túnel do nó (tunnel_id + credentials_json)".to_string()
            })?;
            validate_tunnel_id(&tunnel.tunnel_id)?;
            validate_credentials(&tunnel.credentials_json, &tunnel.tunnel_id)?;
        } else if let Some(tunnel) = &self.tunnel {
            // Settings may exist before a named route is added; still validate.
            validate_tunnel_id(&tunnel.tunnel_id)?;
            validate_credentials(&tunnel.credentials_json, &tunnel.tunnel_id)?;
        }
        Ok(())
    }
}

pub fn validate_binary(raw: &str) -> Result<String, String> {
    let value = raw.trim();
    if value.is_empty() {
        return Err("binário do cloudflared vazio".into());
    }
    if value.chars().count() > 512 {
        return Err("caminho do cloudflared longo demais".into());
    }
    if value.chars().any(|c| c.is_control()) {
        return Err("binário do cloudflared contém caracteres de controle".into());
    }
    Ok(value.to_string())
}

pub fn validate_project(raw: &str) -> Result<String, String> {
    let value = raw.trim();
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() > 63 {
        return Err("projeto deve ter 1-63 caracteres".into());
    }
    if !bytes[0].is_ascii_alphanumeric()
        || !bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Err("projeto deve ser um slug: letras, números, '-', '_' ou '.'".into());
    }
    Ok(value.to_string())
}

/// Validates and normalizes a project's local service URL. Only loopback hosts
/// are accepted; an explicit port is required.
pub fn validate_local_url(raw: &str) -> Result<String, String> {
    let value = raw.trim();
    if value.is_empty() {
        return Err("URL local vazia".into());
    }
    let url = Url::parse(value).map_err(|_| format!("URL local inválida: {value}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("URL local precisa ser http:// ou https://".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("URL local não pode conter usuário/senha".into());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("URL local não pode conter query ou fragmento".into());
    }
    if url.port().is_none() {
        return Err("URL local precisa informar a porta (ex.: http://127.0.0.1:3000)".into());
    }
    let loopback = match url.host() {
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        None => false,
    };
    if !loopback {
        return Err(
            "URL local deve apontar para loopback (127.0.0.1, ::1 ou localhost); exponha o serviço localmente primeiro"
                .into(),
        );
    }
    Ok(url.to_string().trim_end_matches('/').to_string())
}

pub fn validate_hostname(raw: &str) -> Result<String, String> {
    let value = raw.trim().trim_end_matches('.').to_ascii_lowercase();
    if value.is_empty() || value.len() > 253 {
        return Err("hostname inválido".into());
    }
    if value.parse::<IpAddr>().is_ok() {
        return Err("hostname deve ser um nome DNS, não um IP".into());
    }
    let labels: Vec<&str> = value.split('.').collect();
    if labels.len() < 2 {
        return Err("hostname precisa ser um FQDN (ex.: app.projeto.com)".into());
    }
    for label in labels {
        let bytes = label.as_bytes();
        if bytes.is_empty() || bytes.len() > 63 {
            return Err(format!("hostname inválido: rótulo '{label}'"));
        }
        if !bytes[0].is_ascii_alphanumeric() || !bytes[bytes.len() - 1].is_ascii_alphanumeric() {
            return Err(format!("hostname inválido: rótulo '{label}'"));
        }
        if !bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'-')
        {
            return Err(format!("hostname inválido: rótulo '{label}'"));
        }
    }
    Ok(value)
}

pub fn validate_tunnel_id(raw: &str) -> Result<String, String> {
    let value = Uuid::parse_str(raw.trim()).map_err(|_| "tunnel_id precisa ser um UUID".to_string())?;
    Ok(value.to_string())
}

/// Cloudflared credentials are a JSON object with at least `TunnelID` and
/// `TunnelSecret`. Requiring both catches the common mistake of pasting a
/// tunnel *token* instead of the credentials file.
pub fn validate_credentials(raw: &str, tunnel_id: &str) -> Result<(), String> {
    let value: serde_json::Value =
        serde_json::from_str(raw.trim()).map_err(|_| "credentials_json inválido: não é JSON".to_string())?;
    let obj = value
        .as_object()
        .ok_or_else(|| "credentials_json inválido: espere um objeto JSON".to_string())?;
    let credential_tunnel = obj
        .get("TunnelID")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "credentials_json não tem TunnelID".to_string())?;
    if Uuid::parse_str(credential_tunnel).ok() != Uuid::parse_str(tunnel_id.trim()).ok() {
        return Err("TunnelID das credenciais não confere com o tunnel_id".into());
    }
    let secret = obj
        .get("TunnelSecret")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if secret.trim().is_empty() {
        return Err("credentials_json não tem TunnelSecret".into());
    }
    Ok(())
}

#[derive(Serialize)]
struct NamedFile {
    tunnel: String,
    #[serde(rename = "credentials-file")]
    credentials_file: String,
    #[serde(rename = "no-autoupdate")]
    no_autoupdate: bool,
    ingress: Vec<Ingress>,
}

#[derive(Serialize)]
struct Ingress {
    #[serde(skip_serializing_if = "Option::is_none")]
    hostname: Option<String>,
    service: String,
}

/// Renders the config cloudflared reads for a named tunnel. The final catch-all
/// is required by cloudflared and intentionally returns 404 instead of exposing
/// an unpublished host.
pub fn render_named_config(
    tunnel_id: &str,
    credentials_file: &Path,
    routes: &[CloudflaredRouteConfig],
) -> Result<String, String> {
    let tunnel_id = validate_tunnel_id(tunnel_id)?;
    if routes.is_empty() {
        return Err("nenhuma rota named para renderizar".into());
    }
    let mut ingress = Vec::with_capacity(routes.len() + 1);
    for route in routes {
        let hostname = route
            .hostname
            .as_deref()
            .ok_or_else(|| format!("rota '{}' sem hostname", route.project))?;
        let hostname = validate_hostname(hostname)?;
        let service = validate_local_url(&route.local_url)?;
        ingress.push(Ingress {
            hostname: Some(hostname),
            service,
        });
    }
    ingress.push(Ingress {
        hostname: None,
        service: "http_status:404".into(),
    });
    serde_yaml::to_string(&NamedFile {
        tunnel: tunnel_id,
        credentials_file: credentials_file.display().to_string(),
        no_autoupdate: true,
        ingress,
    })
    .map_err(|err| format!("falha ao gerar config YAML: {err}"))
}

/// Extracts the first `https://*.trycloudflare.com` URL from a cloudflared log
/// line. ANSI escapes and box-drawing characters are tolerated.
pub fn extract_quick_tunnel_url(line: &str) -> Option<String> {
    let mut rest = line;
    while let Some(pos) = rest.find("https://") {
        let candidate = &rest[pos..];
        let end = candidate
            .char_indices()
            .find(|(_, c)| !is_url_char(*c))
            .map(|(i, _)| i)
            .unwrap_or(candidate.len());
        let url = candidate[..end].trim_end_matches(['.', ',', ')', ']', ';']);
        if let Some(host) = url
            .strip_prefix("https://")
            .and_then(|s| s.split('/').next())
        {
            if host.ends_with(".trycloudflare.com") {
                return Some(url.to_string());
            }
        }
        rest = &candidate[end.max(1)..];
    }
    None
}

fn is_url_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '-' | '.'
                | '_'
                | '~'
                | ':'
                | '/'
                | '?'
                | '#'
                | '['
                | ']'
                | '@'
                | '!'
                | '$'
                | '&'
                | '\''
                | '('
                | ')'
                | '*'
                | '+'
                | ','
                | ';'
                | '='
                | '%'
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(id: &str, project: &str, url: &str, hostname: Option<&str>) -> CloudflaredRouteConfig {
        CloudflaredRouteConfig {
            id: id.into(),
            project: project.into(),
            local_url: url.into(),
            hostname: hostname.map(str::to_string),
        }
    }

    fn credentials(tunnel: &str) -> String {
        format!(
            r#"{{"AccountTag":"acc","TunnelSecret":"secret","TunnelID":"{tunnel}","Endpoint":""}}"#
        )
    }

    #[test]
    fn loopback_urls_are_normalized_and_remote_hosts_rejected() {
        assert_eq!(
            validate_local_url("http://127.0.0.1:3000/").unwrap(),
            "http://127.0.0.1:3000"
        );
        assert_eq!(
            validate_local_url("https://localhost:8443/app").unwrap(),
            "https://localhost:8443/app"
        );
        assert!(validate_local_url("http://[::1]:8080").is_ok());
        for bad in [
            "http://10.66.0.3:8421",
            "http://192.168.1.2:3000",
            "http://0.0.0.0:3000",
            "ftp://127.0.0.1:21",
            "http://127.0.0.1",
            "http://user:pass@127.0.0.1:3000",
            "http://127.0.0.1:3000?x=1",
            "not a url",
        ] {
            assert!(validate_local_url(bad).is_err(), "aceitou {bad}");
        }
    }

    #[test]
    fn hostnames_are_strict() {
        assert_eq!(
            validate_hostname("App.Example.COM.").unwrap(),
            "app.example.com"
        );
        for bad in ["app", "app..example.com", "-app.example.com", "app-.example.com", "1.2.3.4"] {
            assert!(validate_hostname(bad).is_err(), "aceitou {bad}");
        }
    }

    #[test]
    fn named_config_has_catch_all() {
        let tunnel = "8f0f6d1c-0a7d-4f6e-9d3f-1a2b3c4d5e6f";
        let routes = vec![route("r1", "site", "http://127.0.0.1:3000", Some("app.example.com"))];
        let yaml = render_named_config(tunnel, Path::new("/tmp/creds.json"), &routes).unwrap();
        assert!(yaml.contains(&format!("tunnel: {tunnel}")));
        assert!(yaml.contains("credentials-file: /tmp/creds.json"));
        assert!(yaml.contains("hostname: app.example.com"));
        assert!(yaml.contains("service: http://127.0.0.1:3000"));
        assert!(yaml.contains("service: http_status:404"));
    }

    #[test]
    fn config_validation_requires_tunnel_for_named() {
        let tunnel = Uuid::new_v4().to_string();
        let mut cfg = CloudflaredConfig {
            named: vec![route(
                "r1",
                "site",
                "http://127.0.0.1:3000",
                Some("app.example.com"),
            )],
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
        cfg.tunnel = Some(CloudflaredTunnelConfig {
            tunnel_id: tunnel.clone(),
            credentials_json: credentials(&tunnel),
        });
        cfg.validate().unwrap();
        cfg.named[0].hostname = None;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn credentials_must_match_tunnel_id() {
        let tunnel = Uuid::new_v4().to_string();
        let other = Uuid::new_v4().to_string();
        assert!(validate_credentials(&credentials(&tunnel), &tunnel).is_ok());
        assert!(validate_credentials(&credentials(&other), &tunnel).is_err());
        assert!(validate_credentials("{}", &tunnel).is_err());
        assert!(validate_credentials("not json", &tunnel).is_err());
    }

    #[test]
    fn extracts_quick_url_from_noisy_logs() {
        let line = "\u{1b}[36mINF\u{1b}[0m |  https://brave-cat-likes-music.trycloudflare.com  |";
        assert_eq!(
            extract_quick_tunnel_url(line),
            Some("https://brave-cat-likes-music.trycloudflare.com".into())
        );
        assert_eq!(
            extract_quick_tunnel_url("visit https://brave-cat.trycloudflare.com."),
            Some("https://brave-cat.trycloudflare.com".into())
        );
        assert!(extract_quick_tunnel_url("https://example.com").is_none());
        assert!(extract_quick_tunnel_url("no url here").is_none());
    }
}
