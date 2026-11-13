use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use ipnet::Ipv4Net;
use serde::{Deserialize, Serialize};

use wm_proto::{
    DEFAULT_DNS_ZONE, DEFAULT_MTU, DEFAULT_OVERLAY_CIDR, DEFAULT_STUN_PORT, NetworkInfo,
};

/// Configuration for the control plane (`wm host` / `wm serve`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ControlConfig {
    /// Address the HTTP/TLS listener binds to.
    pub listen: SocketAddr,
    /// Public base URL used in relay tokens, docs and the join command.
    pub public_url: String,
    /// Optional TLS certificate/key. When absent, serve plain HTTP (put a
    /// reverse proxy such as Caddy in front for production).
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
    /// State directory (DB, keys, socket, backups).
    pub data_dir: PathBuf,
    pub network_name: String,
    pub overlay_cidr: String,
    pub dns_zone: String,
    pub mtu: u16,
    pub approval_required: bool,
    pub relay_enabled: bool,
    pub relay_region: String,
    /// Override for the relay public URL (defaults to `public_url`).
    pub relay_url: Option<String>,
    pub stun_enabled: bool,
    pub stun_listen: SocketAddr,
    /// STUN URLs advertised to agents. Defaults to `<public host>:<stun_listen port>`.
    pub stun_urls: Vec<String>,
    /// Initial admin password. When unset a random one is printed once.
    pub admin_password: Option<String>,
}

impl Default for ControlConfig {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:8080".parse().expect("valid addr"),
            public_url: "http://127.0.0.1:8080".into(),
            tls_cert: None,
            tls_key: None,
            data_dir: PathBuf::from("./wm-data"),
            network_name: "wired-magic".into(),
            overlay_cidr: DEFAULT_OVERLAY_CIDR.into(),
            dns_zone: DEFAULT_DNS_ZONE.into(),
            mtu: DEFAULT_MTU,
            approval_required: false,
            relay_enabled: true,
            relay_region: "default".into(),
            relay_url: None,
            stun_enabled: true,
            stun_listen: SocketAddr::from(([0, 0, 0, 0], DEFAULT_STUN_PORT)),
            stun_urls: Vec::new(),
            admin_password: None,
        }
    }
}

impl ControlConfig {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("lendo {}", path.display()))?;
        let mut cfg: ControlConfig =
            toml::from_str(&text).with_context(|| format!("parseando {}", path.display()))?;
        cfg.apply_env();
        // Validation (including derived public endpoints) happens after CLI overrides.
        Ok(cfg)
    }

    pub fn apply_env(&mut self) {
        if let Ok(v) = std::env::var("WM_PUBLIC_URL") {
            self.public_url = v;
        }
        if let Ok(v) = std::env::var("WM_LISTEN") {
            if let Ok(addr) = v.parse() {
                self.listen = addr;
            }
        }
        if let Ok(v) = std::env::var("WM_DATA_DIR") {
            self.data_dir = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("WM_ADMIN_PASSWORD") {
            self.admin_password = Some(v);
        }
        if let Ok(v) = std::env::var("WM_TLS_CERT") {
            self.tls_cert = Some(PathBuf::from(v));
        }
        if let Ok(v) = std::env::var("WM_TLS_KEY") {
            self.tls_key = Some(PathBuf::from(v));
        }
        if let Ok(v) = std::env::var("WM_RELAY_URL") {
            self.relay_url = Some(v);
        }
    }

    pub fn validate(&mut self) -> anyhow::Result<()> {
        let url = url::Url::parse(&self.public_url)
            .with_context(|| format!("public_url inválida: {}", self.public_url))?;
        if url.scheme() != "http" && url.scheme() != "https" {
            bail!("public_url precisa ser http:// ou https://");
        }
        let net: Ipv4Net = self
            .overlay_cidr
            .parse()
            .with_context(|| format!("overlay_cidr inválido: {}", self.overlay_cidr))?;
        if net.prefix_len() > 24 {
            bail!("overlay_cidr precisa ter /24 ou menos para caber a rede");
        }
        if self.mtu < 576 || self.mtu > 1500 {
            bail!("mtu fora da faixa 576..=1500");
        }
        if self.dns_zone.is_empty() || self.dns_zone.contains(' ') {
            bail!("dns_zone inválida");
        }
        if self.tls_cert.is_some() != self.tls_key.is_some() {
            bail!("tls_cert e tls_key devem ser informados juntos");
        }
        if self.stun_urls.is_empty() {
            let host = url.host_str().unwrap_or("127.0.0.1");
            self.stun_urls = vec![format!("stun:{host}:{}", self.stun_listen.port())];
        }
        if self.relay_url.is_none() {
            self.relay_url = Some(self.public_url.trim_end_matches('/').to_string());
        }
        self.relay_url = self
            .relay_url
            .as_ref()
            .map(|u| u.trim_end_matches('/').to_string());
        Ok(())
    }

    pub fn server_ip(&self) -> std::net::Ipv4Addr {
        let net: Ipv4Net = self.overlay_cidr.parse().expect("validated");
        net.hosts().next().expect("network has .1")
    }

    pub fn network_info(&self) -> NetworkInfo {
        NetworkInfo {
            name: self.network_name.clone(),
            overlay_cidr: self.overlay_cidr.parse().expect("validated"),
            dns_zone: self.dns_zone.clone(),
            mtu: self.mtu,
            server_address: self.server_ip(),
        }
    }

    pub fn relay_public_url(&self) -> String {
        format!(
            "{}{}",
            self.relay_url
                .as_deref()
                .unwrap_or(&self.public_url)
                .trim_end_matches('/'),
            wm_proto::DEFAULT_RELAY_PATH
        )
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("control.db")
    }

    pub fn ensure_dirs(&self) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.data_dir)
            .with_context(|| format!("criando {}", self.data_dir.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.data_dir, std::fs::Permissions::from_mode(0o700));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_defaults_are_derived_after_listener_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.toml");
        std::fs::write(&path, "public_url = 'https://mesh.example:8443'\n").unwrap();
        let mut cfg = ControlConfig::load(&path).unwrap();
        assert!(cfg.stun_urls.is_empty());
        cfg.public_url = "https://other.example:9443".into();
        cfg.stun_listen = "0.0.0.0:5349".parse().unwrap();
        cfg.validate().unwrap();
        assert_eq!(cfg.stun_urls, ["stun:other.example:5349"]);
        assert_eq!(cfg.relay_public_url(), "https://other.example:9443/relay");
    }

    #[test]
    fn explicit_stun_advertisement_survives_listener_change() {
        let mut cfg = ControlConfig {
            stun_listen: "0.0.0.0:5349".parse().unwrap(),
            stun_urls: vec!["stun:mesh.example:443".into()],
            ..Default::default()
        };
        cfg.validate().unwrap();
        assert_eq!(cfg.stun_urls, ["stun:mesh.example:443"]);
    }

    #[test]
    fn defaults_are_valid() {
        let mut cfg = ControlConfig::default();
        cfg.validate().unwrap();
        assert_eq!(cfg.server_ip(), "10.66.0.1".parse::<std::net::Ipv4Addr>().unwrap());
        assert_eq!(cfg.relay_public_url(), "http://127.0.0.1:8080/relay");
        assert_eq!(cfg.stun_urls, vec!["stun:127.0.0.1:3478"]);
    }
}
