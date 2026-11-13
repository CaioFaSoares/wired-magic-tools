//! Shared wire types for the wired-magic control plane, agent, CLI, TUI and web UI.
//!
//! Everything in this crate is deliberately transport-agnostic: the control API uses
//! JSON, the relay uses a compact binary framing built from [`RelayFrame`], and the
//! local agent API uses newline-delimited JSON. Keeping one set of types makes the
//! contracts auditable and lets the TUI/web reuse the same data.

use std::collections::BTreeMap;
use std::fmt;
use std::net::{Ipv4Addr, SocketAddr};

use chrono::{DateTime, Utc};
use ipnet::Ipv4Net;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod share;
pub use share::{
    AdminCreateShareRequest, CONNECT_CODE_PREFIX, CreateShareRequest, RenameSelfRequest, ShareInfo,
    ShareKind, ShareProtocol, UpdateShareRequest, decode_connect_code, decode_join_target,
    encode_connect_code, validate_share_description, validate_share_name,
};

pub const PROTOCOL_VERSION: u32 = 1;
pub const API_PREFIX: &str = "/api/v1";
pub const DEFAULT_OVERLAY_CIDR: &str = "10.66.0.0/24";
pub const DEFAULT_DNS_ZONE: &str = "wm.internal";
pub const DEFAULT_MTU: u16 = 1280;
pub const DEFAULT_UDP_PORT: u16 = 41641;
pub const DEFAULT_STUN_PORT: u16 = 3478;
pub const DEFAULT_RELAY_PATH: &str = "/relay";

pub type NodeId = Uuid;

/// WireGuard transport endpoint candidates advertised by a node.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum EndpointKind {
    /// Public mapping learned from STUN.
    Stun,
    /// Address of a local interface, useful for same-LAN peers.
    Lan,
    /// Previously working endpoint cached by the agent.
    Previous,
    /// Explicitly configured endpoint.
    Manual,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct EndpointCandidate {
    pub addr: SocketAddr,
    pub kind: EndpointKind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    #[default]
    Pending,
    Active,
    Disabled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum NatType {
    #[default]
    Unknown,
    Cone,
    Symmetric,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PathKind {
    #[default]
    None,
    Direct,
    Relay,
}

impl fmt::Display for PathKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathKind::None => f.write_str("—"),
            PathKind::Direct => f.write_str("direct"),
            PathKind::Relay => f.write_str("relay"),
        }
    }
}

/// Public view of a node, shared by the map, admin API and integration API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub id: NodeId,
    pub name: String,
    pub hostname: String,
    pub overlay_ip: Ipv4Addr,
    /// base64 (standard, padded) X25519 WireGuard public key.
    pub wg_public_key: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub endpoints: Vec<EndpointCandidate>,
    pub online: bool,
    pub last_seen: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfInfo {
    pub id: NodeId,
    pub name: String,
    pub hostname: String,
    pub overlay_ip: Ipv4Addr,
    pub wg_public_key: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    pub dns_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayInfo {
    /// Base URL, e.g. `https://mesh.example.com` (the `/relay` path is appended).
    pub url: String,
    pub region: String,
    /// Short-lived token signed by the control plane.
    pub token: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkMap {
    pub version: u64,
    pub protocol: u32,
    pub network_name: String,
    pub overlay_cidr: Ipv4Net,
    pub dns_zone: String,
    pub mtu: u16,
    pub me: SelfInfo,
    pub peers: Vec<PeerInfo>,
    pub relay: Option<RelayInfo>,
    #[serde(default)]
    pub stun: Vec<String>,
    pub acl: CompiledAcl,
    /// Desired state of node-side extensions for *this* node. The list is
    /// authoritative: an extension absent from it must stop. It is empty for
    /// agents that do not have the extension feature yet.
    #[serde(default)]
    pub extensions: Vec<ExtensionDesired>,
    /// Named services published on the mesh. Disabled shares stay in the list
    /// so the owner can toggle them from the TUI/GUI; consumers must check
    /// `enabled` before offering them.
    #[serde(default)]
    pub shares: Vec<ShareInfo>,
    /// Loopback services that active extensions ask this node to publish on
    /// its overlay address. Local `--expose` specs are merged by the agent.
    #[serde(default)]
    pub exposes: Vec<MeshExpose>,
    pub server_time: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Extensions
// ---------------------------------------------------------------------------
//
// Extensions are split into a control-plane side (manifest, admin API, data
// ownership) and a node-side sidecar (a process supervised by the agent). The
// map carries only the desired state for the node it was built for; the agent
// reports the observed state through `ReportRequest.extensions`.

/// One extension the node should run. `driver` selects the node-side adapter
/// (`cloudflared`, `exec`, ...); `config` is owned by the extension.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionDesired {
    pub id: String,
    pub name: String,
    pub driver: String,
    #[serde(default)]
    pub version: String,
    /// Monotonic revision for this node's slice. The agent uses it to decide
    /// when to restart a sidecar even if `config` is byte-identical (e.g. an
    /// explicit "restart" action).
    pub revision: u64,
    /// Bumped only when the operator activates/deactivates this extension for
    /// the node. Hooks react to this, not to every config edit.
    #[serde(default)]
    pub activation_revision: u64,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Lifecycle hooks declared by the manifest. Commands run on the node
    /// before/after the driver and are trusted admin content.
    #[serde(default)]
    pub hooks: ExtensionHooks,
    /// Extension-owned JSON. Never interpreted by the agent host.
    #[serde(default)]
    pub config: serde_json::Value,
}

/// One command run by the node at an extension lifecycle boundary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct HookSpec {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default = "default_hook_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub working_dir: Option<String>,
}

pub const DEFAULT_HOOK_TIMEOUT_SECS: u64 = 120;

fn default_hook_timeout() -> u64 {
    DEFAULT_HOOK_TIMEOUT_SECS
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ExtensionHooks {
    /// Run once per (node, extension version); success is remembered under the
    /// agent data directory.
    #[serde(default)]
    pub install: Option<HookSpec>,
    /// Run when the extension becomes active (or its activation revision
    /// changes), before the sidecar starts.
    #[serde(default)]
    pub activate: Option<HookSpec>,
    /// Run when the extension stops being active (or is removed), after the
    /// sidecar stops.
    #[serde(default)]
    pub deactivate: Option<HookSpec>,
}

/// A loopback service exposed on the node's overlay address by an adapter.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MeshExpose {
    pub overlay_port: u16,
    pub local_port: u16,
}

/// Catalog entry shown by the TUI/local GUI for one extension on this node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeExtensionView {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub driver: String,
    #[serde(default)]
    pub ui: String,
    #[serde(default)]
    pub builtin: bool,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub configured: bool,
    #[serde(default)]
    pub activation_revision: u64,
    #[serde(default)]
    pub default_config: bool,
    #[serde(default)]
    pub status: Option<ExtensionStatus>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionState {
    #[default]
    Stopped,
    Starting,
    Running,
    Error,
}

impl fmt::Display for ExtensionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExtensionState::Stopped => f.write_str("stopped"),
            ExtensionState::Starting => f.write_str("starting"),
            ExtensionState::Running => f.write_str("running"),
            ExtensionState::Error => f.write_str("error"),
        }
    }
}

/// Observed state of one node-side extension. `details` is extension-owned
/// (for cloudflared it carries per-route state and public URLs).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExtensionStatus {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub driver: String,
    pub state: ExtensionState,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub public_urls: Vec<String>,
    #[serde(default)]
    pub details: serde_json::Value,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollRequest {
    pub join_token: String,
    pub name: String,
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub version: String,
    /// base64 (standard, padded) X25519 WireGuard public key.
    pub wg_public_key: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub nat_type: NatType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollResponse {
    pub node_id: NodeId,
    pub node_token: String,
    pub overlay_ip: Ipv4Addr,
    pub overlay_cidr: Ipv4Net,
    pub node_name: String,
    pub network_name: String,
    pub control_pubkey: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointsRequest {
    pub udp_port: u16,
    pub candidates: Vec<EndpointCandidate>,
    pub nat_type: NatType,
    pub relay_connected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerStats {
    pub peer_id: NodeId,
    pub path: PathKind,
    pub rtt_ms: Option<u32>,
    pub last_handshake_secs: Option<u64>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub relay_online: bool,
    pub endpoint: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportRequest {
    #[serde(default)]
    pub peers: Vec<PeerStats>,
    pub nat_type: NatType,
    pub relay_connected: bool,
    #[serde(default)]
    pub extensions: Vec<ExtensionStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkInfo {
    pub name: String,
    pub overlay_cidr: Ipv4Net,
    pub dns_zone: String,
    pub mtu: u16,
    pub server_address: Ipv4Addr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionInfo {
    pub version: String,
    pub protocol: u32,
    pub control_pubkey: String,
    pub network: NetworkInfo,
    pub relay: Option<String>,
    #[serde(default)]
    pub stun: Vec<String>,
}

// ---------------------------------------------------------------------------
// ACL
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AclAction {
    #[default]
    Accept,
    Deny,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct PortRange {
    pub start: u16,
    pub end: u16,
}

impl PortRange {
    pub const fn single(p: u16) -> Self {
        Self { start: p, end: p }
    }

    pub fn contains(&self, port: u16) -> bool {
        port >= self.start && port <= self.end
    }
}

/// One compiled rule. Order matters: the first matching rule wins.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompiledRule {
    pub action: AclAction,
    pub src: Ipv4Net,
    pub dst: Ipv4Net,
    /// IP protocol number (6 TCP, 17 UDP, 1 ICMP). `None` means any.
    pub protocol: Option<u8>,
    /// Empty means any port; ignored for ICMP.
    #[serde(default)]
    pub ports: Vec<PortRange>,
    #[serde(default)]
    pub comment: Option<String>,
}

impl CompiledRule {
    pub fn matches(
        &self,
        src: Ipv4Addr,
        dst: Ipv4Addr,
        protocol: u8,
        dst_port: Option<u16>,
    ) -> bool {
        if !self.src.contains(&src) || !self.dst.contains(&dst) {
            return false;
        }
        if let Some(p) = self.protocol {
            if p != protocol {
                return false;
            }
        }
        if protocol == 1 || self.ports.is_empty() {
            return true;
        }
        match dst_port {
            Some(port) => self.ports.iter().any(|range| range.contains(port)),
            None => false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompiledAcl {
    pub version: u32,
    /// When true, no matching rule means allow (the default open policy).
    pub default_allow: bool,
    #[serde(default)]
    pub rules: Vec<CompiledRule>,
}

impl Default for CompiledAcl {
    fn default() -> Self {
        Self {
            version: 0,
            default_allow: true,
            rules: Vec::new(),
        }
    }
}

impl CompiledAcl {
    pub fn is_allowed(
        &self,
        src: Ipv4Addr,
        dst: Ipv4Addr,
        protocol: u8,
        dst_port: Option<u16>,
    ) -> bool {
        for rule in &self.rules {
            if rule.matches(src, dst, protocol, dst_port) {
                return rule.action == AclAction::Accept;
            }
        }
        self.default_allow
    }
}

/// Human-authored policy stored by the control plane. Selectors are resolved when
/// the policy is saved; see `wm-control::acl`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AclPolicy {
    #[serde(default = "default_acl_version")]
    pub version: u32,
    #[serde(default)]
    pub rules: Vec<AclRule>,
}

fn default_acl_version() -> u32 {
    1
}

impl Default for AclPolicy {
    fn default() -> Self {
        Self {
            version: default_acl_version(),
            rules: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AclRule {
    pub action: AclAction,
    /// Selectors: `*`, `name:foo`, `tag:bar`, or a CIDR like `10.66.0.0/24`.
    #[serde(default)]
    pub src: Vec<String>,
    #[serde(default)]
    pub dst: Vec<String>,
    /// `tcp`, `udp`, `icmp`, or `*` (default).
    #[serde(default)]
    pub protocol: Option<String>,
    /// `*`, `22`, `8000-8100`.
    #[serde(default)]
    pub ports: Vec<String>,
    #[serde(default)]
    pub comment: Option<String>,
}

// ---------------------------------------------------------------------------
// Admin API
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminUser {
    pub id: Uuid,
    pub username: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginResponse {
    pub user: AdminUser,
    pub csrf_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeView {
    #[serde(flatten)]
    pub node: PeerInfo,
    pub status: NodeStatus,
    pub created_at: DateTime<Utc>,
    pub approved_at: Option<DateTime<Utc>>,
    pub disabled_at: Option<DateTime<Utc>>,
    pub nat_type: NatType,
    pub relay_connected: bool,
    #[serde(default)]
    pub reported_peers: Vec<PeerStats>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateNodeRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub status: Option<NodeStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateJoinTokenRequest {
    pub name: String,
    #[serde(default = "default_token_ttl")]
    pub ttl_secs: i64,
    #[serde(default = "default_token_uses")]
    pub max_uses: i64,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

fn default_token_ttl() -> i64 {
    24 * 60 * 60
}

fn default_token_uses() -> i64 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinTokenView {
    pub id: Uuid,
    pub name: String,
    pub prefix: String,
    pub tags: Vec<String>,
    pub labels: BTreeMap<String, String>,
    pub max_uses: i64,
    pub uses: i64,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    /// Only present in the creation response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AclView {
    pub policy: AclPolicy,
    pub compiled: CompiledAcl,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulateRequest {
    pub src: String,
    pub dst: String,
    #[serde(default)]
    pub protocol: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulateResponse {
    pub allowed: bool,
    pub rule_index: Option<usize>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Overview {
    pub network: NetworkInfo,
    pub version: u64,
    pub nodes_total: usize,
    pub nodes_online: usize,
    pub nodes_pending: usize,
    pub relay_connected: usize,
    pub direct_pairs: usize,
    pub relay_pairs: usize,
    pub control_uptime_secs: u64,
    pub admin: AdminUser,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEventView {
    pub id: i64,
    pub ts: DateTime<Utc>,
    pub actor: String,
    pub action: String,
    pub target: String,
    pub detail: Option<String>,
    pub ip: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceTokenView {
    pub id: Uuid,
    pub name: String,
    pub scopes: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateServiceTokenRequest {
    pub name: String,
    #[serde(default = "default_service_scope")]
    pub scopes: Vec<String>,
}

fn default_service_scope() -> Vec<String> {
    vec!["integration:nodes:read".to_string()]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsView {
    pub network: NetworkInfo,
    pub approval_required: bool,
    pub relay_url: Option<String>,
    pub stun: Vec<String>,
    pub protocol: u32,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationNode {
    pub id: NodeId,
    pub name: String,
    pub hostname: String,
    pub overlay_ip: Ipv4Addr,
    pub tags: Vec<String>,
    pub labels: BTreeMap<String, String>,
    pub online: bool,
    pub last_seen: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationNodesResponse {
    pub nodes: Vec<IntegrationNode>,
    pub server_time: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ControlEvent {
    NodeUpdated(Box<NodeView>),
    NodeRemoved { id: NodeId },
    JoinTokenCreated(Box<JoinTokenView>),
    JoinTokenRevoked { id: Uuid },
    AclUpdated { version: u32 },
    RelayUpdated,
    Audit(AuditEventView),
    Presence { node_id: NodeId, online: bool },
    /// A node reported a new state for one of its extensions. The payload is
    /// intentionally small; the panel re-reads the extension view on demand.
    ExtensionStatus {
        node_id: NodeId,
        extension_id: String,
        state: ExtensionState,
    },
    /// A share was created or changed (including its enabled flag).
    ShareUpdated(Box<ShareInfo>),
    ShareRemoved { id: Uuid },
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiErrorBody {
    pub error: ApiError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl ApiError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProtoError {
    #[error("invalid relay frame")]
    InvalidRelayFrame,
    #[error("relay frame too large")]
    RelayFrameTooLarge,
}

// ---------------------------------------------------------------------------
// Relay framing
// ---------------------------------------------------------------------------
//
// A relay frame is a single WebSocket binary message. All frames start with a
// one-byte tag; data frames carry the 16-byte peer UUID followed by the opaque
// WireGuard ciphertext. The relay never sees plaintext.

pub const RELAY_FRAME_DATA: u8 = 0x01;
pub const RELAY_FRAME_DELIVER: u8 = 0x02;
pub const RELAY_FRAME_PING: u8 = 0x03;
pub const RELAY_FRAME_PONG: u8 = 0x04;
pub const RELAY_FRAME_PRESENCE: u8 = 0x05;
pub const RELAY_FRAME_ERROR: u8 = 0x06;
pub const RELAY_MAX_PAYLOAD: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayFrame {
    /// Client -> relay: deliver `payload` to `dst`.
    Data { dst: NodeId, payload: Vec<u8> },
    /// Relay -> client: `src` sent a payload.
    Deliver { src: NodeId, payload: Vec<u8> },
    Ping,
    Pong,
    Presence { node: NodeId, online: bool },
    Error(String),
}

impl RelayFrame {
    pub fn encode(&self) -> Result<Vec<u8>, ProtoError> {
        let mut out = Vec::new();
        match self {
            RelayFrame::Data { dst, payload } => {
                if payload.len() > RELAY_MAX_PAYLOAD {
                    return Err(ProtoError::RelayFrameTooLarge);
                }
                out.push(RELAY_FRAME_DATA);
                out.extend_from_slice(dst.as_bytes());
                out.extend_from_slice(payload);
            }
            RelayFrame::Deliver { src, payload } => {
                if payload.len() > RELAY_MAX_PAYLOAD {
                    return Err(ProtoError::RelayFrameTooLarge);
                }
                out.push(RELAY_FRAME_DELIVER);
                out.extend_from_slice(src.as_bytes());
                out.extend_from_slice(payload);
            }
            RelayFrame::Ping => out.push(RELAY_FRAME_PING),
            RelayFrame::Pong => out.push(RELAY_FRAME_PONG),
            RelayFrame::Presence { node, online } => {
                out.push(RELAY_FRAME_PRESENCE);
                out.extend_from_slice(node.as_bytes());
                out.push(u8::from(*online));
            }
            RelayFrame::Error(message) => {
                out.push(RELAY_FRAME_ERROR);
                out.extend_from_slice(message.as_bytes());
            }
        }
        Ok(out)
    }

    pub fn decode(buf: &[u8]) -> Result<Self, ProtoError> {
        let Some((&tag, rest)) = buf.split_first() else {
            return Err(ProtoError::InvalidRelayFrame);
        };
        match tag {
            RELAY_FRAME_DATA => {
                if rest.len() < 16 {
                    return Err(ProtoError::InvalidRelayFrame);
                }
                let dst = Uuid::from_slice(&rest[..16]).map_err(|_| ProtoError::InvalidRelayFrame)?;
                Ok(RelayFrame::Data {
                    dst,
                    payload: rest[16..].to_vec(),
                })
            }
            RELAY_FRAME_DELIVER => {
                if rest.len() < 16 {
                    return Err(ProtoError::InvalidRelayFrame);
                }
                let src = Uuid::from_slice(&rest[..16]).map_err(|_| ProtoError::InvalidRelayFrame)?;
                Ok(RelayFrame::Deliver {
                    src,
                    payload: rest[16..].to_vec(),
                })
            }
            RELAY_FRAME_PING => Ok(RelayFrame::Ping),
            RELAY_FRAME_PONG => Ok(RelayFrame::Pong),
            RELAY_FRAME_PRESENCE => {
                if rest.len() != 17 {
                    return Err(ProtoError::InvalidRelayFrame);
                }
                let node = Uuid::from_slice(&rest[..16]).map_err(|_| ProtoError::InvalidRelayFrame)?;
                Ok(RelayFrame::Presence {
                    node,
                    online: rest[16] != 0,
                })
            }
            RELAY_FRAME_ERROR => Ok(RelayFrame::Error(
                String::from_utf8_lossy(rest).into_owned(),
            )),
            _ => Err(ProtoError::InvalidRelayFrame),
        }
    }
}

// ---------------------------------------------------------------------------
// Relay token claims (signed by the control plane)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayClaims {
    /// Node that may use the relay.
    pub sub: NodeId,
    /// Audience: absolute relay URL the token is valid for.
    pub aud: String,
    pub iat: i64,
    pub exp: i64,
}

// ---------------------------------------------------------------------------
// Local agent API (newline-delimited JSON over a Unix socket)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalRequest {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalResponse {
    pub id: u64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl LocalResponse {
    pub fn ok(id: u64, result: serde_json::Value) -> Self {
        Self {
            id,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: u64, error: impl Into<String>) -> Self {
        Self {
            id,
            ok: false,
            result: None,
            error: Some(error.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStatus {
    pub version: String,
    pub node_id: Option<NodeId>,
    pub node_name: Option<String>,
    pub overlay_ip: Option<Ipv4Addr>,
    pub control_url: Option<String>,
    pub control_connected: bool,
    pub relay_connected: bool,
    pub tun_name: Option<String>,
    pub udp_port: Option<u16>,
    pub nat_type: NatType,
    pub peers_total: usize,
    pub peers_online: usize,
    pub uptime_secs: u64,
    pub map_version: u64,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerStatus {
    pub id: NodeId,
    pub name: String,
    pub overlay_ip: Ipv4Addr,
    pub path: PathKind,
    pub endpoint: Option<String>,
    pub rtt_ms: Option<u32>,
    pub last_handshake_secs: Option<u64>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub relay_online: bool,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PingResult {
    pub peer: String,
    pub overlay_ip: Ipv4Addr,
    pub sent: u32,
    pub received: u32,
    pub rtt_ms_min: Option<f64>,
    pub rtt_ms_avg: Option<f64>,
    pub rtt_ms_max: Option<f64>,
    pub path: PathKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeResult {
    pub peer: String,
    pub overlay_ip: Ipv4Addr,
    pub port: u16,
    pub connected: bool,
    pub elapsed_ms: f64,
    pub path: PathKind,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogLine {
    pub ts: DateTime<Utc>,
    pub level: String,
    pub target: String,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acl_first_match_and_default() {
        let open = CompiledAcl::default();
        assert!(open.is_allowed(
            Ipv4Addr::new(10, 66, 0, 2),
            Ipv4Addr::new(10, 66, 0, 3),
            6,
            Some(22)
        ));

        let acl = CompiledAcl {
            version: 1,
            default_allow: false,
            rules: vec![
                CompiledRule {
                    action: AclAction::Deny,
                    src: "10.66.0.0/24".parse().unwrap(),
                    dst: "10.66.0.3/32".parse().unwrap(),
                    protocol: Some(6),
                    ports: vec![PortRange::single(22)],
                    comment: None,
                },
                CompiledRule {
                    action: AclAction::Accept,
                    src: "10.66.0.0/24".parse().unwrap(),
                    dst: "10.66.0.3/32".parse().unwrap(),
                    protocol: Some(6),
                    ports: vec![PortRange::single(8421)],
                    comment: None,
                },
            ],
        };
        assert!(!acl.is_allowed(
            Ipv4Addr::new(10, 66, 0, 2),
            Ipv4Addr::new(10, 66, 0, 3),
            6,
            Some(22)
        ));
        assert!(acl.is_allowed(
            Ipv4Addr::new(10, 66, 0, 2),
            Ipv4Addr::new(10, 66, 0, 3),
            6,
            Some(8421)
        ));
        assert!(!acl.is_allowed(
            Ipv4Addr::new(10, 66, 0, 2),
            Ipv4Addr::new(10, 66, 0, 3),
            6,
            Some(80)
        ));
    }

    #[test]
    fn relay_frames_roundtrip() {
        let node = Uuid::new_v4();
        let frames = vec![
            RelayFrame::Data {
                dst: node,
                payload: vec![1, 2, 3],
            },
            RelayFrame::Deliver {
                src: node,
                payload: vec![9, 8],
            },
            RelayFrame::Ping,
            RelayFrame::Pong,
            RelayFrame::Presence {
                node,
                online: true,
            },
            RelayFrame::Error("no route".into()),
        ];
        for frame in frames {
            let encoded = frame.encode().unwrap();
            assert_eq!(RelayFrame::decode(&encoded).unwrap(), frame);
        }
        assert!(RelayFrame::decode(&[]).is_err());
        assert!(RelayFrame::decode(&[0xff]).is_err());
    }
}
