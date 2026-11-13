use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use x25519_dalek::{PublicKey, StaticSecret};

use ipnet::Ipv4Net;
use wm_proto::{EnrollRequest, EnrollResponse, NatType};

use crate::control::ControlClient;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExposeSpec {
    /// Porta no endereço overlay (onde os peers conectam).
    pub overlay_port: u16,
    /// Porta no loopback/localhost (serviço real, ex.: Baguette 8421).
    pub local_port: u16,
}

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub data_dir: PathBuf,
    pub control_url: Option<String>,
    pub udp_port: u16,
    pub mtu: u16,
    pub tun_name: Option<String>,
    pub name: Option<String>,
    pub tags: Vec<String>,
    pub labels: BTreeMap<String, String>,
    /// Força todo o tráfego por relay (diagnóstico e redes onde UDP é bloqueado).
    pub force_relay: bool,
    /// Serviços loopback expostos no endereço overlay do nó.
    pub expose: Vec<ExposeSpec>,
}

impl AgentConfig {
    pub fn with_data_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: dir.into(),
            control_url: None,
            udp_port: wm_proto::DEFAULT_UDP_PORT,
            mtu: wm_proto::DEFAULT_MTU,
            tun_name: None,
            name: None,
            tags: Vec::new(),
            labels: BTreeMap::new(),
            force_relay: false,
            expose: Vec::new(),
        }
    }

    pub fn state_path(&self) -> PathBuf {
        self.data_dir.join("agent-state.json")
    }

    pub fn socket_path(&self) -> PathBuf {
        self.data_dir.join("agent.sock")
    }

    pub fn pid_path(&self) -> PathBuf {
        self.data_dir.join("agent.pid")
    }
}

fn default_overlay_cidr() -> Ipv4Net {
    wm_proto::DEFAULT_OVERLAY_CIDR.parse().expect("valid default")
}

pub fn default_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("WM_DATA_DIR") {
        return PathBuf::from(dir);
    }
    #[cfg(target_os = "macos")]
    {
        PathBuf::from("/usr/local/var/wired-magic")
    }
    #[cfg(not(target_os = "macos"))]
    {
        PathBuf::from("/var/lib/wired-magic")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeState {
    pub node_id: Uuid,
    pub node_token: String,
    pub overlay_ip: std::net::Ipv4Addr,
    #[serde(default = "default_overlay_cidr")]
    pub overlay_cidr: Ipv4Net,
    pub wg_private_key: String,
    pub wg_public_key: String,
    pub control_url: String,
    pub node_name: String,
    pub network_name: String,
    #[serde(default)]
    pub expose: Vec<ExposeSpec>,
    pub created_at: DateTime<Utc>,
}

impl NodeState {
    pub fn load(path: &Path) -> anyhow::Result<Option<Self>> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Some(
                serde_json::from_str(&text)
                    .with_context(|| format!("parseando {}", path.display()))?,
            )),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err).with_context(|| format!("lendo {}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn private_key(&self) -> anyhow::Result<StaticSecret> {
        let bytes = STANDARD
            .decode(&self.wg_private_key)
            .context("chave privada inválida")?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("chave privada deve ter 32 bytes"))?;
        Ok(StaticSecret::from(arr))
    }

    pub fn public_key(&self) -> anyhow::Result<PublicKey> {
        Ok(PublicKey::from(&self.private_key()?))
    }
}

pub fn generate_keypair() -> (StaticSecret, PublicKey, String, String) {
    let private = StaticSecret::random_from_rng(rand_core::OsRng);
    let public = PublicKey::from(&private);
    (
        private.clone(),
        public,
        STANDARD.encode(private.to_bytes()),
        STANDARD.encode(public.to_bytes()),
    )
}

/// Hostname used in the admin UI and integration API.
pub fn hostname() -> String {
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

pub fn sanitize_node_name(raw: &str) -> String {
    let mut out: String = raw
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect();
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    out = out.trim_matches(['-', '.', '_']).to_string();
    if out.is_empty() {
        out = "node".into();
    }
    out.truncate(63);
    out
}

/// Enrolls this host and persists the node state. The private key never leaves
/// the machine.
pub async fn join(
    cfg: &AgentConfig,
    control_url: &str,
    join_token: &str,
    name: Option<&str>,
) -> anyhow::Result<NodeState> {
    let (_, _, private_b64, public_b64) = generate_keypair();
    let host = hostname();
    let node_name = sanitize_node_name(
        name.map(str::to_string)
            .or_else(|| cfg.name.clone())
            .unwrap_or_else(|| host.clone())
            .as_str(),
    );
    let req = EnrollRequest {
        join_token: join_token.to_string(),
        name: node_name.clone(),
        hostname: host,
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        wg_public_key: public_b64.clone(),
        labels: cfg.labels.clone(),
        tags: cfg.tags.clone(),
        nat_type: NatType::Unknown,
    };
    let client = ControlClient::new(control_url)?;
    let res: EnrollResponse = client.enroll(&req).await?;
    let state = NodeState {
        node_id: res.node_id,
        node_token: res.node_token,
        overlay_ip: res.overlay_ip,
        overlay_cidr: res.overlay_cidr,
        wg_private_key: private_b64,
        wg_public_key: public_b64,
        control_url: control_url.trim_end_matches('/').to_string(),
        node_name: res.node_name,
        network_name: res.network_name,
        expose: cfg.expose.clone(),
        created_at: Utc::now(),
    };
    state.save(&cfg.state_path())?;
    Ok(state)
}

pub fn require_state(cfg: &AgentConfig) -> anyhow::Result<NodeState> {
    let state = NodeState::load(&cfg.state_path())?;
    match state {
        Some(s) => Ok(s),
        None => bail!(
            "este nó ainda não entrou na malha; rode `wm join {} --token ...`",
            cfg.control_url.as_deref().unwrap_or("<url>")
        ),
    }
}
